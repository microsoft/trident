#!/usr/bin/env python3

import argparse
import libvirt
import os
import subprocess
import xml.etree.ElementTree as ET


def run_command(command: str) -> str:
    result = subprocess.run(
        command, shell=True, capture_output=True, text=True, check=True
    )
    return result.stdout


def check_logfile_for_string(output_log_filepath: str, success_string: str) -> bool:
    with open(output_log_filepath, "r") as file:
        for line in file:
            if success_string in line:
                return True
    return False


def get_domain(vm_name: str) -> libvirt.virDomain:
    conn = libvirt.open("qemu:///system")
    domain = conn.lookupByName(vm_name)
    return domain


def get_xml_element_attribute(vm_name: str, xpath: str, attribute: str) -> str:
    domain = get_domain(vm_name)
    tree = ET.fromstring(domain.XMLDesc())
    xpath_element = tree.find(xpath)
    return xpath_element.attrib[attribute]


def start_domain(vm_name: str):
    domain = get_domain(vm_name)
    domain.createWithFlags(0)


def create_console_connection(vm_name: str) -> libvirt.virStream:
    domain = get_domain(vm_name)
    stream = domain.connect().newStream(0)
    console_flags = libvirt.VIR_DOMAIN_CONSOLE_FORCE | libvirt.VIR_DOMAIN_CONSOLE_SAFE
    domain.openConsole(None, stream, console_flags)
    return stream


def watch_for_usb_iso_login(
    vm_name: str, success_string: str, output_log_filepath: str, log_file_stream
):
    # Create console connection
    stream = create_console_connection(vm_name)
    # Read from console until 'success_string' is found
    while True:
        data_bytes = stream.recv(1024)
        data = data_bytes.decode("utf8", "ignore")
        log_file_stream.write(data)
        log_file_stream.flush()
        if check_logfile_for_string(output_log_filepath, success_string):
            break
    # Close console connection
    stream.finish()


def send_command_to_vm(vm_name, cmd, log_file_stream, output_log_filepath):
    # Create console connection
    stream = create_console_connection(vm_name)

    print(f"Sending '{cmd}'")
    ret = stream.send(f"{cmd}\n".encode("utf-8"))
    print(f"... transmitted '{ret}'")
    # Read from console until 'cmd' is found
    while True:
        data_bytes = stream.recv(1024)
        data = data_bytes.decode("utf8", "ignore")
        log_file_stream.write(data)
        log_file_stream.flush()
        if check_logfile_for_string(output_log_filepath, cmd):
            break
    print(f"... confirmed transmission, '{cmd}' found in {output_log_filepath}")
    # Close console connection
    stream.finish()


def validate_trident_usb_iso(vm_name: str, output_log_file: str):
    if os.path.exists(output_log_file):
        # Clean log files from any previous run
        os.remove(output_log_file)

    with open(f"{output_log_file}", "a") as log_file_stream:
        # start VM
        print(f"Start VM: {vm_name}")
        start_domain(vm_name)

        # get disk device
        target_device = get_xml_element_attribute(
            vm_name, "./devices/disk[@device='disk']/target", "dev"
        )
        print(f"Find target device for {vm_name}: {target_device}")

        # get serial pts device
        serial_pts_device = get_xml_element_attribute(
            vm_name, "./devices/console[@type='pty']/source", "path"
        )
        print(f"Find serial port for {vm_name}: {serial_pts_device}")

        print(f"Wait for usb-iso to be ready.")
        watch_for_usb_iso_login(
            vm_name,
            "azl-installer-mos login: root (automatic login)",
            output_log_file,
            log_file_stream,
        )
        print(f"... usb-iso to has started.")

        print(
            f"Send CTRL-C to VM over serial for some reason to re-enable serial input."
        )
        run_command(f"echo -e '\x03' > {serial_pts_device}")
        print(f"... CTRL-C sent.")

        print(f"Modify embedded config.yaml to reflect {target_device}.")
        send_command_to_vm(
            vm_name,
            f"sed -i 's|device: /dev/nvme0n1|device: /dev/{target_device}|' /etc/trident/config.yaml",
            log_file_stream,
            output_log_file,
        )

        print(f"Start OS installation")
        # Open a serial console connection
        send_command_to_vm(vm_name, "trident run", log_file_stream, output_log_file)

        print(f"Wait while new OS is installing.")
        watch_for_usb_iso_login(
            vm_name, "trident-testimg login:", output_log_file, log_file_stream
        )
        print(f"... finished installing new OS.")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--vm-name",
        default="usb-iso-test-vm",
        help="VM name",
    )
    parser.add_argument(
        "--log",
        default="/tmp/test.log",
        help="Serial output log file",
    )
    args = parser.parse_args()

    validate_trident_usb_iso(
        args.vm_name,
        args.log,
    )


if __name__ == "__main__":
    main()
