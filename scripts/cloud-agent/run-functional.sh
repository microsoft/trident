#!/usr/bin/env bash

set -euo pipefail

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly JUNIT_XML="target/trident_functional_tests.xml"
build_output=""
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

main() {
    local mode="${1:-smoke}"
    local functional_image
    local marker_expression
    local requested_namespace
    local virtdeploy_network
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

    for command in az virsh qemu-system-x86_64 python3; do
        if ! command -v "$command" >/dev/null 2>&1; then
            echo "Missing $command. Run scripts/cloud-agent/setup-functional.sh first." >&2
            return 1
        fi
    done
    test -e /dev/kvm
    run virsh -c qemu:///system list --all

    requested_namespace="${TRIDENT_FUNCTIONAL_NAMESPACE:-trident-functional-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-$$}"
    virtdeploy_network="${TRIDENT_FUNCTIONAL_NETWORK:-$(
        python3 -c \
            'import hashlib, sys; digest = hashlib.sha256(sys.argv[1].encode()).digest(); print(f"10.{digest[0]}.{digest[1]}.0/24")' \
            "$requested_namespace"
    )}"

    printf '\n==> python3 scripts/cloud-agent/download_functional_artifact.py\n'
    functional_image="$(python3 scripts/cloud-agent/download_functional_artifact.py)"
    test -f "$functional_image"

    build_output="$(mktemp)"
    trap cleanup EXIT

    run bash -c "cd tools && go generate pkg/tridentgrpc/grpc.go"
    run make \
        "BUILD_OUTPUT=$build_output" \
        build-functional-test-cc \
        generate-functional-test-manifest \
        bin/virtdeploy
    chmod +x bin/virtdeploy
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
