#!/bin/bash
# regen-sshd-keys is a one-shot service that generates SSH host keys in
# /var/srv on first boot. Enable it via wants symlink because the generic
# `services.enable` in MIC config is reserved for systemd unit names that
# come from packages, and our unit is delivered via additionalFiles.
ln -sf /etc/systemd/system/regen-sshd-keys.service \
  /etc/systemd/system/multi-user.target.wants/regen-sshd-keys.service
