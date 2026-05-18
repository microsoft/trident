#!/bin/bash
# Defensive enable of trident.service.
#
# AZL3 gets this via the trident-service RPM's %systemd_post scriptlet.
# AZL4 doesn't have that RPM yet, so we ship the unit via additionalFiles
# and *should* be able to rely on baseimg-grub-azl4.yaml's `services.enable:`
# stanza. In practice, `services.enable` did not create the
# multi-user.target.wants/trident.service symlink in MIC AZL4 builds
# (build 1120959 showed multi-user.target reached but trident.service
# never started post-reboot, leaving servicingState stuck at
# ab-update-finalized). Until we figure out why, manually link the unit
# so the post-reboot commit oneshot fires.
set -euxo pipefail

mkdir -p /etc/systemd/system/multi-user.target.wants
ln -sf /usr/lib/systemd/system/trident.service \
    /etc/systemd/system/multi-user.target.wants/trident.service

# Belt and braces: log the enabled state for diagnostics. systemctl is-enabled
# may fail inside MIC's chroot without a running dbus, so don't gate the
# script on it.
systemctl is-enabled trident.service 2>&1 || true
ls -l /etc/systemd/system/multi-user.target.wants/trident.service || true
