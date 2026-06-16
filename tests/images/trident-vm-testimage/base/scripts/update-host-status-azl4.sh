#!/bin/bash
# AZL4 equivalent of AZL3's update-host-status.sh.
#
# Runs inside MIC's chroot at qcow2 build time. Populates the trident
# datastore with the host status derived from Prism's history.json so
# the system boots ready for storm-trident to drive A/B updates -- no
# first-boot bootstrap, no datastore creation at runtime.
#
# Mirrors AZL3's pattern (scripts/update-host-status.sh, called from
# baseimg-grub.yaml). The trident binary in the chroot must understand
# that `--disk /dev/sda` is the runtime label and not a build-time
# existence assertion; see trident PR fixing the spurious check in
# crates/trident/src/init/offline/mod.rs.
set -euxo pipefail

trident offline-initialize
