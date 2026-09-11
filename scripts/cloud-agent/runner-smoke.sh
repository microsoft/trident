#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <log-directory>" >&2
    exit 2
fi

log_dir="$1"
mkdir -p "$log_dir"

run_id="${GITHUB_RUN_ID:-local}"
run_attempt="${GITHUB_RUN_ATTEMPT:-0}"
domain_name="trident-cloud-agent-smoke-${run_id}-${run_attempt}"
domain_xml="$log_dir/domain.xml"
qemu_connection="qemu:///system"

cleanup() {
    if sudo virsh --connect "$qemu_connection" dominfo "$domain_name" >/dev/null 2>&1; then
        sudo virsh --connect "$qemu_connection" destroy "$domain_name" >/dev/null
    fi
}
trap cleanup EXIT

{
    echo "Timestamp: $(date --iso-8601=seconds)"
    echo "Runner: ${RUNNER_NAME:-unknown}"
    echo "Runner OS: ${RUNNER_OS:-unknown}"
    echo "Runner architecture: ${RUNNER_ARCH:-unknown}"
    echo "GitHub run: $run_id"
    echo "GitHub attempt: $run_attempt"
    echo "User: $(id)"
    echo "Kernel: $(uname -a)"
} | tee "$log_dir/environment.txt"

lscpu | tee "$log_dir/lscpu.txt"
free -h | tee "$log_dir/memory.txt"
df -hT | tee "$log_dir/filesystems.txt"
ls -l /dev/kvm | tee "$log_dir/kvm-device.txt"

if [[ ! -c /dev/kvm ]]; then
    echo "/dev/kvm is not a character device" >&2
    exit 1
fi

sudo test -r /dev/kvm
sudo test -w /dev/kvm

if [[ -r /sys/module/kvm_intel/parameters/nested ]]; then
    cat /sys/module/kvm_intel/parameters/nested | tee "$log_dir/nested-virtualization.txt"
elif [[ -r /sys/module/kvm_amd/parameters/nested ]]; then
    cat /sys/module/kvm_amd/parameters/nested | tee "$log_dir/nested-virtualization.txt"
else
    echo "No nested virtualization module parameter found" | tee "$log_dir/nested-virtualization.txt"
fi

qemu_bin="$(command -v qemu-system-x86_64)"
"$qemu_bin" --version | tee "$log_dir/qemu-version.txt"
sudo virsh --connect "$qemu_connection" version | tee "$log_dir/virsh-version.txt"
sudo virsh --connect "$qemu_connection" nodeinfo | tee "$log_dir/libvirt-nodeinfo.txt"

set +e
sudo virt-host-validate qemu 2>&1 | tee "$log_dir/virt-host-validate.txt"
virt_validate_status=${PIPESTATUS[0]}
set -e
echo "$virt_validate_status" > "$log_dir/virt-host-validate.exit-code"
if [[ $virt_validate_status -ne 0 ]]; then
    echo "virt-host-validate reported a host validation failure" >&2
    exit "$virt_validate_status"
fi

set +e
sudo timeout 5s "$qemu_bin" \
    -machine q35,accel=kvm \
    -cpu host \
    -m 256 \
    -display none \
    -monitor none \
    -serial none \
    -nodefaults \
    -S \
    >"$log_dir/qemu-kvm-probe.txt" 2>&1
qemu_status=$?
set -e

echo "$qemu_status" > "$log_dir/qemu-kvm-probe.exit-code"
if [[ $qemu_status -ne 124 && $qemu_status -ne 143 ]]; then
    cat "$log_dir/qemu-kvm-probe.txt" >&2
    echo "QEMU did not remain running with KVM acceleration" >&2
    exit 1
fi

cat > "$domain_xml" <<EOF
<domain type='kvm'>
  <name>${domain_name}</name>
  <memory unit='MiB'>256</memory>
  <currentMemory unit='MiB'>256</currentMemory>
  <vcpu placement='static'>1</vcpu>
  <os>
    <type arch='x86_64' machine='q35'>hvm</type>
  </os>
  <features>
    <acpi/>
    <apic/>
  </features>
  <cpu mode='host-passthrough' check='none'/>
  <clock offset='utc'/>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>destroy</on_reboot>
  <on_crash>destroy</on_crash>
  <devices>
    <emulator>${qemu_bin}</emulator>
    <controller type='pci' model='pcie-root'/>
    <memballoon model='none'/>
  </devices>
</domain>
EOF

sudo virsh --connect "$qemu_connection" create --paused "$domain_xml"
sudo virsh --connect "$qemu_connection" dominfo "$domain_name" | tee "$log_dir/domain-info.txt"
sudo virsh --connect "$qemu_connection" destroy "$domain_name"

if sudo virsh --connect "$qemu_connection" dominfo "$domain_name" >/dev/null 2>&1; then
    echo "Transient smoke domain still exists after destroy" >&2
    exit 1
fi

cat > "$log_dir/result.txt" <<EOF
PASS
KVM acceleration accepted by QEMU.
Libvirt created and destroyed transient domain ${domain_name}.
virt-host-validate exit code: ${virt_validate_status}
EOF

cat "$log_dir/result.txt"
