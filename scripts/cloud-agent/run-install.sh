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
readonly UPDATE_TIMEOUT="${TRIDENT_UPDATE_TIMEOUT:-20m}"
readonly UPDATE_CONFIG_PATH="/var/lib/trident/config.yaml"
readonly UPDATE_VERSION="2"
readonly SSH_ATTEMPTS="${TRIDENT_INSTALL_SSH_ATTEMPTS:-60}"
readonly SSH_DELAY_SECONDS="${TRIDENT_INSTALL_SSH_DELAY_SECONDS:-5}"

runtime_dir=""
log_dir=""
namespace=""
vm_name=""
netlisten_pid=""
target_host_ip=""
ssh_key_path=""

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
        if [[ -n "$netlisten_pid" ]]; then
            ps -fp "$netlisten_pid" || true
        fi
        if [[ -n "$target_host_ip" && -f "$ssh_key_path" ]]; then
            ssh \
                -i "$ssh_key_path" \
                -o BatchMode=yes \
                -o ConnectTimeout=10 \
                -o StrictHostKeyChecking=no \
                -o UserKnownHostsFile=/dev/null \
                "testing-user@$target_host_ip" \
                "sudo /usr/bin/trident get; sudo journalctl --no-pager -u trident.service -n 200" ||
                true
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
    if [[ -n "$netlisten_pid" ]] && kill -0 "$netlisten_pid" 2>/dev/null; then
        kill "$netlisten_pid" || cleanup_code=$?
        wait "$netlisten_pid" || true
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

wait_for_http() {
    local url="$1"
    local attempt

    for ((attempt = 1; attempt <= 30; attempt++)); do
        if python3 - "$url" <<'PY'
import sys
import urllib.request

request = urllib.request.Request(sys.argv[1], method="HEAD")
with urllib.request.urlopen(request, timeout=5) as response:
    if response.status != 200:
        raise RuntimeError(f"unexpected HTTP status {response.status}")
PY
        then
            return
        fi
        echo "Waiting for update artifact server ($attempt/30)"
        sleep 1
    done
    echo "Update artifact server did not become ready at $url" >&2
    return 1
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

assert_host_status() {
    local status_path="$1"
    local expected_state="$2"
    local expected_volume="$3"
    local servicing_state
    local active_volume

    servicing_state="$(
        awk '$1 == "servicingState:" { print $2; exit }' "$status_path"
    )"
    if [[ "$servicing_state" != "$expected_state" ]]; then
        echo "Expected servicingState $expected_state, got: $servicing_state" >&2
        return 1
    fi

    active_volume="$(
        awk '$1 == "abActiveVolume:" { print $2; exit }' "$status_path"
    )"
    if [[ "$active_volume" != "$expected_volume" ]]; then
        echo "Expected abActiveVolume $expected_volume, got: $active_volume" >&2
        return 1
    fi
}

main() {
    local mode="${1:-install}"
    if (($# > 1)) || [[ "$mode" != "install" && "$mode" != "update" ]]; then
        echo "Usage: scripts/cloud-agent/run-install.sh [install|update]" >&2
        return 2
    fi

    cd "$REPO_ROOT"
    reexec_with_libvirt_group "$@"
    trap cleanup EXIT

    require_command az
    require_command make
    require_command python3
    require_command scp
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
    runtime_dir="$(mktemp -d "/tmp/trident-${mode}-${run_label}-XXXXXX")"
    chmod 0700 "$runtime_dir"
    namespace="trident-${mode}-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-$$"

    local provenance_path
    provenance_path="$(
        TRIDENT_INSTALL_CACHE_DIR="$INSTALL_CACHE_DIR" \
            python3 scripts/cloud-agent/download_install_artifacts.py
    )"
    cp "$provenance_path" "$log_dir/provenance.json"

    make -j2 target/release/trident bin/netlaunch bin/virtdeploy
    if [[ "$mode" == "update" ]]; then
        make -j2 bin/netlisten bin/storm-trident
    fi
    test -x "$CANDIDATE_TRIDENT"
    test -x "$CANDIDATE_LAUNCHER"
    test -x bin/netlaunch
    test -x bin/virtdeploy
    if [[ "$mode" == "update" ]]; then
        test -x bin/netlisten
        test -x bin/storm-trident
    fi

    local candidate_sha
    candidate_sha="$(sha256sum "$CANDIDATE_TRIDENT" | awk '{print $1}')"
    git rev-parse HEAD >"$log_dir/candidate-source-commit.txt"
    printf '%s\n' "$candidate_sha" >"$log_dir/candidate-trident-sha256.txt"

    local host_config="$runtime_dir/trident-config.yaml"
    local ssh_key="$runtime_dir/id_ed25519"
    ssh_key_path="$ssh_key"
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
    target_host_ip="$host_ip"

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
        tee "$log_dir/host-status-initial.yaml"
    ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo cat /etc/os-release; sudo findmnt -no SOURCE /" |
        tee "$log_dir/target-system-initial.txt"

    local initial_remote_sha
    initial_remote_sha="$(
        ssh "${ssh_options[@]}" "testing-user@$host_ip" \
            "sudo sha256sum /usr/bin/trident" |
            awk '{print $1}'
    )"
    if [[ "$initial_remote_sha" != "$candidate_sha" ]]; then
        echo "Installed Trident SHA-256 does not match the candidate binary" >&2
        echo "candidate: $candidate_sha" >&2
        echo "installed: $initial_remote_sha" >&2
        exit 1
    fi
    printf '%s\n' "$initial_remote_sha" \
        >"$log_dir/installed-trident-sha256-initial.txt"

    local initial_selinux_state
    initial_selinux_state="$(
        ssh "${ssh_options[@]}" "testing-user@$host_ip" "sudo getenforce"
    )"
    printf '%s\n' "$initial_selinux_state" >"$log_dir/selinux-state-initial.txt"
    if [[ "$initial_selinux_state" != "Enforcing" ]]; then
        echo "Expected SELinux Enforcing, got: $initial_selinux_state" >&2
        exit 1
    fi

    assert_host_status "$log_dir/host-status-initial.yaml" provisioned volume-a

    if [[ "$mode" == "install" ]]; then
        python3 - "$log_dir/provenance.json" "$candidate_sha" "$initial_remote_sha" "$host_ip" "$log_dir" <<'PY'
import json
import sys

provenance = json.load(open(sys.argv[1], encoding="utf-8"))
payloads = {
    artifact["payload"]["archivePath"]: artifact["payload"]["sha256"]
    for artifact in provenance["artifacts"]
}
print("Phase C1 fresh-install smoke test passed")
print(f"ADO build ID: {provenance['build']['id']}")
print(f"Pinned source commit: {provenance['build']['sourceVersion']}")
print(
    "Installer ISO SHA-256: "
    f"{payloads['trident-direct-streaming-installer-amd64/trident-direct-streaming-installer-amd64.iso']}"
)
print(
    "Install COSI SHA-256: "
    f"{payloads['trident-testimage/trident-testimage_0.cosi']}"
)
print(f"Candidate Trident SHA-256: {sys.argv[2]}")
print(f"Installed Trident SHA-256: {sys.argv[3]}")
print(f"Target IP: {sys.argv[4]}")
print("SELinux: Enforcing")
print("servicingState: provisioned")
print("abActiveVolume: volume-a")
print(f"Logs: {sys.argv[5]}")
PY
        return
    fi

    local current_config="$runtime_dir/trident-current-config.yaml"
    local update_config="$runtime_dir/trident-update-config.yaml"
    ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo /usr/bin/trident get configuration" >"$current_config"

    local update_url
    update_url="$(
        python3 scripts/cloud-agent/prepare_update_host_config.py \
            "$current_config" \
            "$update_config" \
            --version "$UPDATE_VERSION"
    )"
    "$CANDIDATE_TRIDENT" validate "$update_config" -v info
    cp "$current_config" "$log_dir/trident-config-before-update.yaml"
    cp "$update_config" "$log_dir/trident-config-update.yaml"
    printf '%s\n' "$update_url" >"$log_dir/update-image-url.txt"

    local update_port
    update_port="$(
        python3 -c \
            'import sys,urllib.parse; parsed=urllib.parse.urlsplit(sys.argv[1]); print(parsed.port or (443 if parsed.scheme == "https" else 80))' \
            "$update_url"
    )"
    bin/netlisten \
        --port "$update_port" \
        --servefolder "$COSI_DIRECTORY" \
        --trace-file "$log_dir/trident-update-metrics.jsonl" \
        --full-logstream "$log_dir/logstream-update-full.log" \
        >"$log_dir/netlisten-update.log" 2>&1 &
    netlisten_pid=$!
    wait_for_http "$update_url"

    scp "${ssh_options[@]}" \
        "$update_config" \
        "testing-user@$host_ip:/tmp/trident-update-config.yaml"
    ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo install -m 0644 -o root -g root /tmp/trident-update-config.yaml '$UPDATE_CONFIG_PATH' && rm -f /tmp/trident-update-config.yaml"

    timeout "$UPDATE_TIMEOUT" \
        ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo /usr/bin/trident grpc-client update -v trace '$UPDATE_CONFIG_PATH' --allowed-operations stage" \
        2>&1 | tee "$log_dir/stage-ab-update.log"

    ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo /usr/bin/trident get" |
        tee "$log_dir/host-status-staged.yaml"
    assert_host_status "$log_dir/host-status-staged.yaml" ab-update-staged volume-a

    local staged_remote_sha
    staged_remote_sha="$(
        ssh "${ssh_options[@]}" "testing-user@$host_ip" \
            "sudo sha256sum /usr/bin/trident" |
            awk '{print $1}'
    )"
    if [[ "$staged_remote_sha" != "$candidate_sha" ]]; then
        echo "Staged host Trident SHA-256 does not match the candidate binary" >&2
        echo "candidate: $candidate_sha" >&2
        echo "staged host: $staged_remote_sha" >&2
        exit 1
    fi
    printf '%s\n' "$staged_remote_sha" \
        >"$log_dir/installed-trident-sha256-staged.txt"

    local staged_selinux_state
    staged_selinux_state="$(
        ssh "${ssh_options[@]}" "testing-user@$host_ip" "sudo getenforce"
    )"
    printf '%s\n' "$staged_selinux_state" >"$log_dir/selinux-state-staged.txt"
    if [[ "$staged_selinux_state" != "Enforcing" ]]; then
        echo "Expected SELinux Enforcing after stage, got: $staged_selinux_state" >&2
        exit 1
    fi

    timeout "$UPDATE_TIMEOUT" bin/storm-trident helper ab-update \
        "$ssh_key" \
        "$host_ip" \
        testing-user \
        host \
        --trident-config "$UPDATE_CONFIG_PATH" \
        --version "$UPDATE_VERSION" \
        --finalize-ab-update \
        2>&1 | tee "$log_dir/finalize-ab-update.log"

    wait_for_ssh "$host_ip" "$ssh_key"
    ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo /usr/bin/trident get" |
        tee "$log_dir/host-status-final.yaml"
    assert_host_status "$log_dir/host-status-final.yaml" provisioned volume-b
    ssh "${ssh_options[@]}" "testing-user@$host_ip" \
        "sudo cat /etc/os-release; sudo findmnt -no SOURCE /" |
        tee "$log_dir/target-system-final.txt"

    local final_remote_sha
    final_remote_sha="$(
        ssh "${ssh_options[@]}" "testing-user@$host_ip" \
            "sudo sha256sum /usr/bin/trident" |
            awk '{print $1}'
    )"
    if [[ "$final_remote_sha" != "$candidate_sha" ]]; then
        echo "Updated Trident SHA-256 does not match the candidate binary" >&2
        echo "candidate: $candidate_sha" >&2
        echo "updated: $final_remote_sha" >&2
        exit 1
    fi
    printf '%s\n' "$final_remote_sha" \
        >"$log_dir/installed-trident-sha256-final.txt"

    local final_selinux_state
    final_selinux_state="$(
        ssh "${ssh_options[@]}" "testing-user@$host_ip" "sudo getenforce"
    )"
    printf '%s\n' "$final_selinux_state" >"$log_dir/selinux-state-final.txt"
    if [[ "$final_selinux_state" != "Enforcing" ]]; then
        echo "Expected SELinux Enforcing after update, got: $final_selinux_state" >&2
        exit 1
    fi

    python3 - "$log_dir/provenance.json" "$candidate_sha" "$initial_remote_sha" "$staged_remote_sha" "$final_remote_sha" "$host_ip" "$log_dir" <<'PY'
import json
import sys

provenance = json.load(open(sys.argv[1], encoding="utf-8"))
payloads = {
    artifact["payload"]["archivePath"]: artifact["payload"]["sha256"]
    for artifact in provenance["artifacts"]
}
print("Phase C2 A/B update smoke test passed")
print(f"ADO build ID: {provenance['build']['id']}")
print(f"Pinned source commit: {provenance['build']['sourceVersion']}")
print(
    "Installer ISO SHA-256: "
    f"{payloads['trident-direct-streaming-installer-amd64/trident-direct-streaming-installer-amd64.iso']}"
)
print(
    "Install COSI SHA-256: "
    f"{payloads['trident-testimage/trident-testimage_0.cosi']}"
)
print(
    "Update COSI SHA-256: "
    f"{payloads['trident-testimage/trident-testimage_1.cosi']}"
)
print(f"Candidate Trident SHA-256: {sys.argv[2]}")
print(f"Initial installed Trident SHA-256: {sys.argv[3]}")
print(f"Staged host Trident SHA-256: {sys.argv[4]}")
print(f"Final installed Trident SHA-256: {sys.argv[5]}")
print(f"Target IP: {sys.argv[6]}")
print("Initial state: provisioned, volume-a")
print("Staged state: ab-update-staged, volume-a")
print("Final state: provisioned, volume-b")
print("SELinux before, during, and after update: Enforcing")
print(f"Logs: {sys.argv[7]}")
PY
}

main "$@"
