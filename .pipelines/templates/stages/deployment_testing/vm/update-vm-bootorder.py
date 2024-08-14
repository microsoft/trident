import os
import subprocess
import xml.etree.ElementTree as ET

VM_NAME = "virtdeploy-vm-0"
XML_FILE = f"/tmp/{VM_NAME}.xml"


def run_command(command):
    result = subprocess.run(command, shell=True, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"Command '{command}' failed with error: {result.stderr}")
    return result.stdout


# Dump the current XML configuration to a temporary file
run_command(f"sudo virsh dumpxml {VM_NAME} > {XML_FILE}")

# Parse the XML file
tree = ET.parse(XML_FILE)
root = tree.getroot()

# Remove the <boot order='1'/> line from the cdrom device
for disk in root.findall("./devices/disk"):
    if disk.get("device") == "cdrom":
        boot = disk.find("boot")
        if boot is not None and boot.get("order") == "1":
            disk.remove(boot)

# Add <boot order='1'/> to the sda device
for disk in root.findall("./devices/disk"):
    source = disk.find("source")
    if (
        disk.get("device") == "disk"
        and source is not None
        and source.get("file")
        == "/var/lib/libvirt/images/virtdeploy-pool/virtdeploy-vm-0-0-volume.qcow2"
    ):
        boot = ET.Element("boot", order="1")
        disk.append(boot)

# Write the updated XML back to the file
tree.write(XML_FILE)

# Define the updated XML configuration
run_command(f"sudo virsh define {XML_FILE}")

# Cleanup
os.remove(XML_FILE)

print(f"Boot order updated successfully for VM: {VM_NAME}")
