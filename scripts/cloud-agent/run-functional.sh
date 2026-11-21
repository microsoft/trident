#!/usr/bin/env bash

set -euo pipefail

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly JUNIT_XML="target/trident_functional_tests.xml"
build_output=""
pytest_cache_dir=""
virtdeploy_namespace=""

usage() {
    cat <<'EOF'
Usage: scripts/cloud-agent/run-functional.sh [smoke|full]

smoke runs VM deployment plus the positive helper functional tests.
full runs the complete functional test suite.
EOF
}

run() {
    printf '\n==> %s\n' "$*"
    "$@"
}

cleanup() {
    local exit_code=$?
    local cleanup_code=0

    trap - EXIT
    set +e
    if [[ -n "$build_output" ]]; then
        rm -f -- "$build_output"
    fi
    if [[ -n "$virtdeploy_namespace" && -x "$REPO_ROOT/bin/virtdeploy" ]]; then
        "$REPO_ROOT/bin/virtdeploy" clean \
            --namespace "$virtdeploy_namespace" || cleanup_code=$?
    fi
    if [[ -n "$pytest_cache_dir" ]]; then
        rm -rf -- "$pytest_cache_dir" || cleanup_code=$?
    fi
    if [[ "$exit_code" -eq 0 && "$cleanup_code" -ne 0 ]]; then
        exit_code=$cleanup_code
    fi
    exit "$exit_code"
}

enter_libvirt_group() {
    local libvirt_gid
    local command
    local argument

    libvirt_gid="$(getent group libvirt 2>/dev/null | cut -d: -f3 || true)"
    if [[ -z "$libvirt_gid" ]]; then
        echo "Missing libvirt group. Run scripts/cloud-agent/setup-functional.sh first." >&2
        return 1
    fi
    if [[ " $(id -G) " == *" $libvirt_gid "* ]]; then
        return
    fi

    command="exec bash"
    for argument in "$REPO_ROOT/scripts/cloud-agent/run-functional.sh" "$@"; do
        printf -v command '%s %q' "$command" "$argument"
    done
    exec sg libvirt -c "$command"
}

acquire_host_lock() {
    local lock_path="/var/lib/trident-cloud-agent/functional.lock"

    if [[ ! -f "$lock_path" || -L "$lock_path" ]]; then
        echo "Missing or unsafe functional lock. Run scripts/cloud-agent/setup-functional.sh first." >&2
        return 1
    fi
    # Setup owns the directory; libvirt members can read but cannot replace it.
    # Read-only open avoids O_CREAT and fs.protected_regular across runner UIDs.
    exec {host_lock}<"$lock_path"
    if ! flock -n "$host_lock"; then
        echo "Host is busy with another functional run" >&2
        return 1
    fi
    if [[ -L "$lock_path" || ! -f "$lock_path" ||
          ! "$lock_path" -ef "/proc/$$/fd/$host_lock" ||
          "$(stat -Lc %h "/proc/$$/fd/$host_lock")" != 1 ]]; then
        echo "Functional lock inode changed or is unsafe; refusing to run" >&2
        return 1
    fi
}

main() {
    local mode="${1:-smoke}"
    local functional_image
    local marker_expression
    local requested_namespace
    local virtdeploy_network
    local host_lock
    local -a network_args
    local -a test_paths

    case "$mode" in
        smoke)
            marker_expression="(helpers and positive) or not functional"
            test_paths=(
                tests/functional_tests/test_setup.py
                tests/functional_tests/ft.json
            )
            ;;
        full)
            marker_expression=""
            test_paths=(
                tests/functional_tests/test_setup.py
                tests/functional_tests/
            )
            ;;
        -h | --help)
            usage
            return
            ;;
        *)
            usage >&2
            return 2
            ;;
    esac

    enter_libvirt_group "$@"

    cd "$REPO_ROOT"
    export PATH="$HOME/.local/bin:$PATH"
    export LIBVIRT_DEFAULT_URI=qemu:///system

    for command in az flock ip virsh qemu-system-x86_64 python3 stat; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "Missing $command. Run scripts/cloud-agent/setup-functional.sh first." >&2
            return 1
        fi
    done
    test -e /dev/kvm
    run virsh -c qemu:///system list --all

    requested_namespace="${TRIDENT_FUNCTIONAL_NAMESPACE:-trident-functional-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-$$}"
    if [[ ! "$requested_namespace" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]*$ ]]; then
        echo "Namespace must contain only letters, digits, _ or -" >&2
        return 1
    fi
    # Generators, reports and subnet selection are shared even across namespaces.
    # Keep this descriptor open through the EXIT cleanup and never unlink the lock.
    acquire_host_lock
    network_args=(
        python3 scripts/cloud-agent/select_functional_network.py
        "$requested_namespace"
    )
    if [[ -n "${TRIDENT_FUNCTIONAL_NETWORK:-}" ]]; then
        network_args+=(--network "$TRIDENT_FUNCTIONAL_NETWORK")
    fi
    printf '\n==> python3 scripts/cloud-agent/download_functional_artifact.py\n'
    functional_image="$(python3 scripts/cloud-agent/download_functional_artifact.py)"
    test -f "$functional_image"

    build_output="$(mktemp)"
    trap cleanup EXIT
    pytest_cache_dir="$(mktemp -d)"

    run bash -c "cd tools && go generate pkg/tridentgrpc/grpc.go"
    run make \
        "BUILD_OUTPUT=$build_output" \
        build-functional-test-cc \
        generate-functional-test-manifest \
        bin/virtdeploy
    chmod +x bin/virtdeploy
    virtdeploy_network="$("${network_args[@]}")"
    virtdeploy_namespace="$requested_namespace"

    pytest_args=(
        python3 -u -m pytest
        --color=yes
        --log-level=INFO
        --force-upload
        "${test_paths[@]}"
        --keep-duplicates
        -v
        -o junit_logging=all
        -o "cache_dir=$pytest_cache_dir"
        --junitxml "$JUNIT_XML"
        --build-output "$build_output"
        --functional-base-image "$functional_image"
        --keep-environment
        --virtdeploy-network "$virtdeploy_network"
        --virtdeploy-namespace "$virtdeploy_namespace"
    )
    if [[ -n "$marker_expression" ]]; then
        pytest_args+=(-m "$marker_expression")
    fi
    run "${pytest_args[@]}"
}

main "$@"
