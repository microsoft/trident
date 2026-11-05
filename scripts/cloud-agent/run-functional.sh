#!/usr/bin/env bash

set -euo pipefail

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly TEST_DIR="/tmp/trident-test"
readonly JUNIT_XML="target/trident_functional_tests.xml"

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
    local marker_expression
    local build_output
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

    run python3 scripts/cloud-agent/download_functional_artifact.py

    build_output="$(mktemp)"
    trap 'rm -f "$build_output"' EXIT

    run make \
        "BUILD_OUTPUT=$build_output" \
        build-functional-test-cc \
        generate-functional-test-manifest \
        bin/virtdeploy
    chmod +x bin/virtdeploy

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
        --test-dir "$TEST_DIR"
        --build-output "$build_output"
    )
    if [[ -n "$marker_expression" ]]; then
        pytest_args+=(-m "$marker_expression")
    fi
    run "${pytest_args[@]}"
}

main "$@"
