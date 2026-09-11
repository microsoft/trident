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
    if virsh --connect "$qemu_connection" dominfo "$domain_name" >/dev/null 2>&1; then
        virsh --connect "$qemu_connection" destroy "$domain_name" >/dev/null
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
ls -l /dev/kvm /run/libvirt/libvirt-sock | tee "$log_dir/device-permissions.txt"
getfacl -p /dev/kvm /run/libvirt/libvirt-sock | tee "$log_dir/device-acls.txt"

if [[ ! -c /dev/kvm ]]; then
    echo "/dev/kvm is not a character device" >&2
    exit 1
fi

if [[ -r /sys/module/kvm_intel/parameters/nested ]]; then
    nested_parameter="/sys/module/kvm_intel/parameters/nested"
elif [[ -r /sys/module/kvm_amd/parameters/nested ]]; then
    nested_parameter="/sys/module/kvm_amd/parameters/nested"
else
    echo "No nested virtualization module parameter found" |
        tee "$log_dir/nested-virtualization.txt" >&2
    exit 1
fi

nested_value="$(cat "$nested_parameter")"
printf '%s\n' "$nested_value" | tee "$log_dir/nested-virtualization.txt"
case "$nested_value" in
    1 | Y | y) ;;
    *)
        echo "Nested virtualization is not enabled: $nested_value" >&2
        exit 1
        ;;
esac

qemu_bin="$(command -v qemu-system-x86_64)"
"$qemu_bin" --version | tee "$log_dir/qemu-version.txt"
virsh --connect "$qemu_connection" version | tee "$log_dir/virsh-version.txt"
virsh --connect "$qemu_connection" nodeinfo | tee "$log_dir/libvirt-nodeinfo.txt"

set +e
virt-host-validate qemu 2>&1 | tee "$log_dir/virt-host-validate.txt"
virt_validate_status=${PIPESTATUS[0]}
set -e
echo "$virt_validate_status" > "$log_dir/virt-host-validate.exit-code"
if [[ $virt_validate_status -ne 0 ]]; then
    echo "virt-host-validate returned $virt_validate_status; direct QEMU and libvirt probes determine the result" >&2
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

virsh --connect "$qemu_connection" create --paused "$domain_xml"
virsh --connect "$qemu_connection" dominfo "$domain_name" | tee "$log_dir/domain-info.txt"
virsh --connect "$qemu_connection" destroy "$domain_name"

if virsh --connect "$qemu_connection" dominfo "$domain_name" >/dev/null 2>&1; then
    echo "Transient smoke domain still exists after destroy" >&2
    exit 1
fi

cat > "$log_dir/result.txt" <<EOF
PASS
The runner user created and destroyed KVM domain ${domain_name} through libvirt.
virt-host-validate exit code: ${virt_validate_status} (informational)
EOF

cat "$log_dir/result.txt"
