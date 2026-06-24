#!/bin/bash
# Defensive enable of trident.service and tridentd.socket.
#
# AZL3 gets these via the trident-service RPM's %systemd_post scriptlet.
# AZL4 doesn't have that RPM yet, so we ship the units via additionalFiles
# and *should* be able to rely on baseimg-grub-azl4.yaml's `services.enable:`
# stanza. In practice, `services.enable` did not create the
# multi-user.target.wants/trident.service symlink in MIC AZL4 builds
# (build 1120959 showed multi-user.target reached but trident.service
# never started post-reboot, leaving servicingState stuck at
# ab-update-finalized). Until we figure out why, manually link the
# units defensively.
#
# tridentd.socket gets the same treatment because (a) if services.enable
# is unreliable for one unit, it's likely unreliable for the other, and
# (b) storm-trident drives every update/commit/rollback through the
# tridentd gRPC socket — a missing /run/trident/trident.sock at boot
# would fail every subsequent storm-trident invocation in the test
# pipeline.
set -euxo pipefail

mkdir -p /etc/systemd/system/multi-user.target.wants
mkdir -p /etc/systemd/system/sockets.target.wants
ln -sf /usr/lib/systemd/system/trident.service \
    /etc/systemd/system/multi-user.target.wants/trident.service
ln -sf /usr/lib/systemd/system/tridentd.socket \
    /etc/systemd/system/sockets.target.wants/tridentd.socket

# Belt and braces: log the enabled state for diagnostics. systemctl is-enabled
# may fail inside MIC's chroot without a running dbus, so don't gate the
# script on it.
systemctl is-enabled trident.service 2>&1 || true
systemctl is-enabled tridentd.socket 2>&1 || true
ls -l /etc/systemd/system/multi-user.target.wants/trident.service || true
ls -l /etc/systemd/system/sockets.target.wants/tridentd.socket || true
