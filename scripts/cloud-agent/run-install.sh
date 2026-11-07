#!/usr/bin/env bash

set -euo pipefail

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly DATA_DISK_MOUNT="/mnt/storage"
readonly INSTALL_CACHE_DIR="${TRIDENT_INSTALL_CACHE_DIR:-$DATA_DISK_MOUNT/trident-cloud-agent/install}"
readonly INSTALL_LOG_ROOT="${TRIDENT_INSTALL_LOG_DIR:-$DATA_DISK_MOUNT/trident-cloud-agent/install-runs}"
readonly HOST_CONFIG_SOURCE="$REPO_ROOT/tests/e2e_tests/trident_configurations/base/trident-config.yaml"
readonly INSTALLER_ISO="$REPO_ROOT/artifacts/iso/trident-direct-streaming-installer-amd64.iso"
readonly COSI_DIRECTORY="$REPO_ROOT/artifacts/test-image"
readonly CANDIDATE_TRIDENT="$REPO_ROOT/target/release/trident"
readonly CANDIDATE_LAUNCHER="$REPO_ROOT/target/release/trident-acl-agent"
readonly CANDIDATE_SELINUX_POLICY_DIR="$REPO_ROOT/packaging/selinux-policy-trident"
readonly INSTALL_TIMEOUT="${TRIDENT_INSTALL_TIMEOUT:-30m}"
readonly SSH_ATTEMPTS="${TRIDENT_INSTALL_SSH_ATTEMPTS:-60}"
readonly SSH_DELAY_SECONDS="${TRIDENT_INSTALL_SSH_DELAY_SECONDS:-5}"

runtime_dir=""
log_dir=""
namespace=""
vm_name=""

reexec_with_libvirt_group() {
    if [[ "${TRIDENT_INSTALL_LIBVIRT_GROUP:-0}" == "1" ]]; then
        return
    fi
    if id -nG | tr ' ' '\n' | grep -Fxq libvirt; then
        return
    fi

    local quoted_command
    printf -v quoted_command ' %q' "$0" "$@"
    exec sg libvirt -c "TRIDENT_INSTALL_LIBVIRT_GROUP=1${quoted_command}"
}

capture_diagnostics() {
    {
        echo "=== install failure diagnostics ==="
        date --iso-8601=seconds
        id
        uname -a
        findmnt --mountpoint "$DATA_DISK_MOUNT" || true
        ls -l /dev/kvm /run/libvirt/libvirt-sock 2>&1 || true
        virsh list --all || true
        virsh net-list --all || true
        if [[ -n "$vm_name" ]]; then
            virsh dominfo "$vm_name" || true
            virsh dumpxml "$vm_name" || true
        fi
        sudo journalctl --no-pager -u libvirtd.service -n 200 || true
        sudo journalctl --no-pager -u virtqemud.service -n 200 || true
    } 2>&1 | tee "$log_dir/diagnostics.log"
}

cleanup() {
    local exit_code=$?
    local cleanup_code=0
    trap - EXIT
    set +e

    if ((exit_code != 0)) && [[ -n "$log_dir" ]]; then
        capture_diagnostics
    fi
    if [[ -n "$namespace" && -x "$REPO_ROOT/bin/virtdeploy" ]]; then
        "$REPO_ROOT/bin/virtdeploy" clean --namespace "$namespace" \
            > >(tee "$log_dir/cleanup.log") 2>&1 ||
            cleanup_code=$?
    fi
    if [[ -n "$runtime_dir" && -d "$runtime_dir" ]]; then
        rm -rf "$runtime_dir"
    fi
    if ((exit_code == 0 && cleanup_code != 0)); then
        exit_code=$cleanup_code
    fi
    exit "$exit_code"
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Missing $1. Run scripts/cloud-agent/setup-install.sh first." >&2
        exit 1
    fi
}

wait_for_ssh() {
    local host_ip="$1"
    local key_path="$2"
    local attempt
    local ssh_options=(
        -i "$key_path"
        -o BatchMode=yes
        -o ConnectTimeout=10
        -o StrictHostKeyChecking=no
        -o UserKnownHostsFile=/dev/null
    )

    for ((attempt = 1; attempt <= SSH_ATTEMPTS; attempt++)); do
        if ssh "${ssh_options[@]}" "testing-user@$host_ip" true; then
            return
        fi
        echo "Waiting for target SSH ($attempt/$SSH_ATTEMPTS)"
        sleep "$SSH_DELAY_SECONDS"
    done
    echo "Target SSH did not become ready at $host_ip" >&2
    return 1
}

main() {
    cd "$REPO_ROOT"
    reexec_with_libvirt_group "$@"
    trap cleanup EXIT

    require_command az
    require_command make
    require_command python3
    require_command ssh
    require_command timeout
    require_command virsh

    if ! mountpoint --quiet "$DATA_DISK_MOUNT"; then
        echo "Expected runner data disk is not mounted at $DATA_DISK_MOUNT" >&2
        exit 1
    fi
    if [[ ! -c /dev/kvm ]]; then
        echo "Nested virtualization is unavailable: /dev/kvm is missing" >&2
        exit 1
    fi
    if [[ ! -S /run/libvirt/libvirt-sock ]]; then
        echo "libvirt socket is unavailable at /run/libvirt/libvirt-sock" >&2
        exit 1
    fi

    local run_label
    run_label="${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-$(date -u +%Y%m%dT%H%M%SZ)-$$"
    log_dir="$INSTALL_LOG_ROOT/$run_label"
    mkdir -p "$log_dir"
    runtime_dir="$(mktemp -d "/tmp/trident-c1-${run_label}-XXXXXX")"
    chmod 0700 "$runtime_dir"
    namespace="trident-c1-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-$$"

    local provenance_path
    provenance_path="$(
        TRIDENT_INSTALL_CACHE_DIR="$INSTALL_CACHE_DIR" \
            python3 scripts/cloud-agent/download_install_artifacts.py
    )"
    cp "$provenance_path" "$log_dir/provenance.json"

    make -j2 target/release/trident bin/netlaunch bin/virtdeploy
    test -x "$CANDIDATE_TRIDENT"
    test -x "$CANDIDATE_LAUNCHER"
    test -x bin/netlaunch
    test -x bin/virtdeploy

    local candidate_sha
    candidate_sha="$(sha256sum "$CANDIDATE_TRIDENT" | awk '{print $1}')"
    git rev-parse HEAD >"$log_dir/candidate-source-commit.txt"
    printf '%s\n' "$candidate_sha" >"$log_dir/candidate-trident-sha256.txt"

    local host_config="$runtime_dir/trident-config.yaml"
    local ssh_key="$runtime_dir/id_ed25519"
    cp "$HOST_CONFIG_SOURCE" "$host_config"
    python3 tests/e2e_tests/helpers/edit_host_config.py \
        --keypath "$ssh_key" \
        --hostconfig "$host_config" \
        --runtimeEnv host
    python3 scripts/cloud-agent/prepare_install_host_config.py \
        "$host_config" \
        "$CANDIDATE_SELINUX_POLICY_DIR"
    chmod 0600 "$ssh_key"
    "$CANDIDATE_TRIDENT" validate "$host_config" -v info
    cp "$host_config" "$log_dir/trident-config.yaml"

    local netlaunch_config="$runtime_dir/netlaunch.yaml"
    local virtdeploy_status="$log_dir/virtdeploy.json"
    bin/virtdeploy create-one \
        --namespace "$namespace" \
        --mem 12 \
        --disks 32,32 \
        --netlaunch "$netlaunch_config" \
        --json >"$virtdeploy_status"

    vm_name="$(
        python3 -c \
            'import json,sys; print(json.load(open(sys.argv[1]))["vms"][0]["name"])' \
            "$virtdeploy_status"
    )"
    local host_ip
    host_ip="$(
        python3 -c \
            'import json,sys; print(json.load(open(sys.argv[1]))["vms"][0]["ip"])' \
            "$virtdeploy_status"
    )"

    timeout "$INSTALL_TIMEOUT" bin/netlaunch \
        --iso "$INSTALLER_ISO" \
        --config "$netlaunch_config" \
        --trident "$host_config" \
        --servefolder "$COSI_DIRECTORY" \
        --logstream \
        --remoteaddress "$log_dir/remote-address.txt" \
        --trace-file "$log_dir/trident-clean-install-metrics.jsonl" \
        --full-logstream "$log_dir/logstream-full.log" \
        --wait-for-provisioned-state \
        --rcp-agent-mode cli \
        --trident-binary "$CANDIDATE_TRIDENT" \
        --launcher-binary "$CANDIDATE_LAUNCHER" \
        2>&1 | tee "$log_dir/clean-install-deployment.log"

    wait_for_ssh "$host_ip" "$ssh_key"

    local ssh_options=(
        -i "$ssh_key"
        -o BatchMode=yes
        -o ConnectTimeout=10
        -o StrictHostKeyChecking=no
        -o UserKnownHostsFile=/dev/null
    )
    ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo /usr/bin/trident get" |
        tee "$log_dir/host-status.yaml"
    ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo cat /etc/os-release; sudo findmnt -no SOURCE /" |
        tee "$log_dir/target-system.txt"

    local remote_sha
    remote_sha="$(
        ssh "${ssh_options[@]}" "testing-user@$host_ip" \
            "sudo sha256sum /usr/bin/trident" |
            awk '{print $1}'
    )"
    if [[ "$remote_sha" != "$candidate_sha" ]]; then
        echo "Installed Trident SHA-256 does not match the candidate binary" >&2
        echo "candidate: $candidate_sha" >&2
        echo "installed: $remote_sha" >&2
        exit 1
    fi
    printf '%s\n' "$remote_sha" >"$log_dir/installed-trident-sha256.txt"

    local servicing_state
    servicing_state="$(
        awk '$1 == "servicingState:" { print $2; exit }' "$log_dir/host-status.yaml"
    )"
    if [[ "$servicing_state" != "provisioned" ]]; then
        echo "Expected servicingState provisioned, got: $servicing_state" >&2
        exit 1
    fi

    local active_volume
    active_volume="$(
        awk '$1 == "abActiveVolume:" { print $2; exit }' "$log_dir/host-status.yaml"
    )"
    if [[ "$active_volume" != "volume-a" ]]; then
        echo "Expected abActiveVolume volume-a, got: $active_volume" >&2
        exit 1
    fi

    python3 - "$log_dir/provenance.json" "$candidate_sha" "$remote_sha" "$host_ip" "$log_dir" <<'PY'
import json
import sys

provenance = json.load(open(sys.argv[1], encoding="utf-8"))
payloads = {
    artifact["name"]: artifact["payload"]["sha256"]
    for artifact in provenance["artifacts"]
}
print("Phase C1 fresh-install smoke test passed")
print(f"ADO build ID: {provenance['build']['id']}")
print(f"Pinned source commit: {provenance['build']['sourceVersion']}")
print(
    "Installer ISO SHA-256: "
    f"{payloads['trident-direct-streaming-installer-amd64']}"
)
print(f"Install COSI SHA-256: {payloads['trident-testimage']}")
print(f"Candidate Trident SHA-256: {sys.argv[2]}")
print(f"Installed Trident SHA-256: {sys.argv[3]}")
print(f"Target IP: {sys.argv[4]}")
print("servicingState: provisioned")
print("abActiveVolume: volume-a")
print(f"Logs: {sys.argv[5]}")
PY
}

main "$@"
