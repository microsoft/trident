#!/bin/bash
# AZL4 doesn't ship a `hostname` binary in `coreutils` (Fedora moved it to
# its own package which AZL4 hasn't picked up yet). The pytest E2E
# framework uses `hostname` as a smoke test of the SSH session in
# tests/e2e_tests/conftest.py, so without this shim every test errors out
# at fixture setup.
#
# Tiny POSIX-only replacement that reads /etc/hostname, plus a passthrough
# for `hostname -s` and `hostname -f` for completeness.
case "$1" in
    -s|--short)
        cat /etc/hostname | cut -d. -f1
        ;;
    -f|--fqdn|"")
        cat /etc/hostname
        ;;
    *)
        cat /etc/hostname
        ;;
esac
