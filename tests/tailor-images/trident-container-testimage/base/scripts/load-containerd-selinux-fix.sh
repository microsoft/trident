#!/bin/bash
# Load SELinux policy module that grants the 'map' permission on
# container_runtime_t:file for container engines.  This fixes containerd
# 2.2+ MountManager bbolt mmap() denials under enforcing mode.
semodule -i /usr/share/selinux/packages/containerd-mmap-fix.cil
