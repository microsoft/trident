#!/usr/bin/env bash

set -euo pipefail

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

usage() {
    cat <<'EOF'
Usage: scripts/cloud-agent/validate.sh [all|format|rust|go]

Runs source-only validation that does not require VM image artifacts.
The default group is "all".
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

main() {
    cd "$REPO_ROOT"

    case "${1:-all}" in
        all)
            validate_format
            validate_rust
            validate_go
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
