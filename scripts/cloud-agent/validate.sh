#!/usr/bin/env bash

set -euo pipefail

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly TAILOR_RUST_TOOLCHAIN="1.98.1"

usage() {
    cat <<'EOF'
Usage: scripts/cloud-agent/validate.sh [all|format|rust|go|tailor|cloud-agent|functional]

The default "all" group runs source-only validation. The explicit "functional"
group downloads the pinned VM artifact and runs the bounded functional smoke suite.
EOF
}

run() {
    printf '\n==> %s\n' "$*"
    "$@"
}

validate_format() {
    run cargo fmt -- --check
    run python3 -m black --check . --exclude "azure-linux-image-tools"

    mapfile -d '' go_files < <(find tools -type f -name '*.go' -print0)
    local unformatted
    unformatted="$(gofmt -l -s "${go_files[@]}")"
    if [[ -n "$unformatted" ]]; then
        printf 'Go files require formatting:\n%s\n' "$unformatted" >&2
        return 1
    fi
}

validate_rust() {
    run make check
    run make validate-api-schema
    run make test
}

validate_go() {
    (
        cd tools
        run go generate pkg/rcp/tlscerts/certs.go
        run go generate pkg/tridentgrpc/grpc.go
        run go test -mod=readonly ./...
    )
    (
        cd tools/installer
        # Go 1.25 vet rejects a pre-existing logrus Errorf call in this module.
        run go test -mod=readonly -vet=off ./...
    )
}

validate_tailor() (
    local scratch_dir
    scratch_dir="$(mktemp -d)"
    trap 'rm -rf "$scratch_dir"' EXIT

    # Tailor uses public crates.io. Copy its self-contained workspace outside
    # the repository so both Cargo and nested Cargo commands from build scripts
    # cannot inherit Trident's private crates.io replacement.
    tar --exclude=target -C "$REPO_ROOT/tools/tailor" -cf - . |
        tar -C "$scratch_dir" -xf -
    cd "$scratch_dir"
    run cargo "+$TAILOR_RUST_TOOLCHAIN" fmt --all --check
    run cargo "+$TAILOR_RUST_TOOLCHAIN" clippy \
        --workspace \
        --all-targets \
        --locked \
        -- \
        -D warnings
    run cargo "+$TAILOR_RUST_TOOLCHAIN" test \
        --workspace \
        --locked
)

validate_cloud_agent() {
    run python3 -m unittest discover \
        -s scripts/cloud-agent/tests \
        -p 'test_*.py'
}

validate_functional() {
    run bash scripts/cloud-agent/run-functional.sh smoke
}

main() {
    cd "$REPO_ROOT"

    case "${1:-all}" in
        all)
            validate_format
            validate_rust
            validate_go
            validate_tailor
            validate_cloud_agent
            ;;
        format)
            validate_format
            ;;
        rust)
            validate_rust
            ;;
        go)
            validate_go
            ;;
        tailor)
            validate_tailor
            ;;
        cloud-agent)
            validate_cloud_agent
            ;;
        functional)
            validate_functional
            ;;
        -h | --help)
            usage
            ;;
        *)
            usage >&2
            return 2
            ;;
    esac
}

main "$@"
