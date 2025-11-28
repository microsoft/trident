import json
import logging
import os
from argparse import (
    ArgumentParser,
    Namespace,
    ArgumentDefaultsHelpFormatter,
    RawTextHelpFormatter,
)
from pathlib import Path
import re
import subprocess
from typing import List

from virtdeploy import DEFAULT_METADATA_FILE

from virtdeploy.helpers import (
    LibvirtHelper,
    VirtualMachineTemplate,
    ConfigFlags,
    ConfigHelper,
)
from virtdeploy.helpers.vmmetadata import CloudInitConfig
from virtdeploy.utils import (
    SubCommand,
    main_location,
    make_file,
    silentremove,
    get_host_default_gateway_interface,
)
from virtdeploy.utils.network import get_host_default_gateway_interface_ip

log = logging.getLogger(__name__)


def init(parser: ArgumentParser):
    parser.add_argument(
        "--network",
        default="192.168.242.0/24",
        help="Network address to connect VMs to. Format is X.X.X.X/Y",
    )

    parser.add_argument(
        "-o",
        "--out",
        help="Path to write VM metadata to.",
        default=os.path.join(main_location(), DEFAULT_METADATA_FILE),
    )

    parser.add_argument(
        "--clean",
        action="store_true",
        help="Cleanup ALL local libvirt resources in the namespace and exit.",
    )

    parser.add_argument(
        "-c", "--cpus", help="Default virtual CPUs for each node.", default=4
    )
    parser.add_argument(
        "-m", "--mem", help="Default allocated RAM for each node in GiB.", default=6
    )
    parser.add_argument(
        "-d",
        "--disks",
        help="Default disk sizes for each node in GiB (comma separated list).",
        default="32",
    )
    parser.add_argument(
        "--os-disk",
        dest="os_disk",
        help="Path to an existing qcow2 disk image to use as the OS disk for the VMs.",
        default=None,
        type=Path,
        metavar="PATH",
    )

    ci_grp = parser.add_argument_group("cloud-init", "Cloud-init options")
    ci_grp.add_argument(
        "--ci-user",
        dest="cloud_init_userdata",
        help="Path to a cloud-init user-data file to use for the VMs.",
        default=None,
        type=Path,
        metavar="PATH",
    )

    ci_grp.add_argument(
        "--ci-meta",
        dest="cloud_init_metadata",
        help="Path to a cloud-init meta-data file to use for the VMs.",
        default=None,
        type=Path,
        metavar="PATH",
    )

    parser.add_argument(
        "nodes",
        help="Node definition.",
        metavar="flags[:cpus[:mem[:disks]]]",
        nargs="*",
        default=[":"],
    )

    parser.add_argument(
        "--dry-run",
        action="store_true",
        dest="dryrun",
        help="Don't actually create/destroy anything.",
    )

    parser.add_argument(
        "--netlaunch",
        help="Path to write netlaunch config yaml file to.",
        default=os.path.join(main_location(), "vm-netlaunch.yaml"),
    )


class VMSpecParser:
    # Flags to be set when not explicitly specified
    DEFAULT_FLAGS = ConfigFlags.NONE | ConfigFlags.EMULATED_TPM

    # Flags that are used as a base, therefore always set
    BASE_FLAGS = ConfigFlags.NONE

    BASE_REGEX = r"([{}]*)(:(\d*))?(:(\d*))?(:((\d+,?)*))?"

    GRP_F = 1
    GRP_C = 3
    GRP_M = 5
    GRP_D = 7

    def __init__(self, value: str) -> None:
        # dynamically update the regex based on the available flags
        compiled = re.compile(
            VMSpecParser.BASE_REGEX.format("".join(ConfigFlags.flag_dict().keys()))
        )
        m = compiled.fullmatch(value)
        if m is None:
            raise Exception(f"Could not parse spec: {value}")

        try:
            # parse flags
            self.flags = ConfigFlags.from_str(
                m.group(VMSpecParser.GRP_F),
                default=VMSpecParser.DEFAULT_FLAGS,
                base=VMSpecParser.BASE_FLAGS,
            )
        except ValueError as ex:
            raise Exception(f"Could not parse flags: {ex}")
        self.cpus = m.group(VMSpecParser.GRP_C)
        self.mem = m.group(VMSpecParser.GRP_M)
        self.disks = m.group(VMSpecParser.GRP_D)

    def _nullEmptyElseDefault(self, value: str, default: int) -> int:
        if value == None or value == "":
            return default
        return int(value)

    def _nullEmptyElseDefaultList(self, value: str, default: str) -> int:
        if value == None or value == "":
            return [int(x) for x in default.split(",")]
        return [int(x) for x in value.split(",")]

    def getFlags(self) -> ConfigFlags:
        return self.flags

    def getCpus(self, default: int) -> int:
        return self._nullEmptyElseDefault(self.cpus, default)

    def getMem(self, default: int) -> int:
        return self._nullEmptyElseDefault(self.mem, default)

    def getDisks(self, default: str) -> int:
        disks = self._nullEmptyElseDefaultList(self.disks, default)
        diskCountLimit = ord("z") - ord("a") + 1
        if len(disks) > diskCountLimit:
            log.critical(f"Too many disks, limit is {diskCountLimit}")
            exit(1)
        return disks


def run(args: Namespace):
    nodes: List[str] = args.nodes
    if len(nodes) < 1:
        log.critical("Zero nodes specified!")
        exit(1)

    if args.os_disk:
        if not args.os_disk.exists():
            log.critical(f"OS disk does not exist: {args.os_disk}")
            exit(1)
        if not args.os_disk.is_file():
            log.critical(f"OS disk is not a file: {args.os_disk}")
            exit(1)
        if not args.os_disk.suffix == ".qcow2":
            log.critical(f"OS disk is not a qcow2 file: {args.os_disk}")
            exit(1)

    cloud_init_config = None
    ci_user = args.cloud_init_userdata
    ci_meta = args.cloud_init_metadata
    if ci_user or ci_meta:
        if ci_user is None:
            log.critical(
                "Cloud-init user-data file is required if metadata is provided."
            )
            exit(1)
        if ci_meta is None:
            log.critical(
                "Cloud-init metadata file is required if user-data is provided."
            )
            exit(1)
        if not ci_user.exists():
            log.critical(f"Cloud-init user-data file does not exist: {ci_user}")
            exit(1)
        if not ci_user.is_file():
            log.critical(f"Cloud-init user-data is not a file: {ci_user}")
            exit(1)
        if not ci_meta.exists():
            log.critical(f"Cloud-init metadata file does not exist: {ci_meta}")
            exit(1)
        if not ci_meta.is_file():
            log.critical(f"Cloud-init metadata is not a file: {ci_meta}")
            exit(1)
        cloud_init_config = CloudInitConfig(
            userdata=ci_user,
            metadata=ci_meta,
        )

    vmtemplates: List[VirtualMachineTemplate] = []
    cores = set()
    for i, node in enumerate(nodes):
        try:
            p = VMSpecParser(node)
            vmtemplates.append(
                VirtualMachineTemplate(
                    f"{args.nameprefix}-vm-{i}",
                    p.getFlags(),
                    p.getCpus(args.cpus),
                    p.getMem(args.mem),
                    p.getDisks(args.disks),
                    args.os_disk,
                    cloud_init_config,
                )
            )
        except Exception as ex:
            log.critical(f"Failed to parse: {node}")
            log.critical(f"Error: {ex}")
            exit(1)

    # Setup Libvirt
    lv = LibvirtHelper(
        args.nameprefix,
        args.network,
        vmtemplates,
        get_host_default_gateway_interface(),
        False,
    )

    if args.dryrun:
        action = "CREATING" if not args.clean else "REMOVING"
        log.info(f"DRY RUN ({action})")
        for vmt in vmtemplates:
            print(vmt)
        return

    # We only want to clean!
    if args.clean:
        lv.clean()
        silentremove(args.out)
        return

    # Contruct all resources
    vms = lv.construct()

    for vm in vms:
        log.info(f"Created: {vm.name} [{vm.UUIDString}]")

    # Save VM metadata
    vmdata = {
        "nameprefix": args.nameprefix,
        "virtualnetwork": args.network,
        "virtualmachines": [vm.export_data() for vm in vms],
    }
    out_file = make_file(args.out)
    with open(out_file, "w", encoding="utf8") as f:
        json.dump(vmdata, f, indent=4)

    # Create nvram files for each VM with 666 permissions
    log.info("Creating NVRAM files for each VM")
    nvram_dir = Path("/var/lib/libvirt/qemu/nvram")
    subprocess.run(["sudo", "mkdir", "-p", nvram_dir], check=True)
    subprocess.run(["sudo", "chmod", "o+rx", "/var/lib/libvirt"], check=True)
    subprocess.run(["sudo", "chmod", "o+rx", "/var/lib/libvirt/qemu"], check=True)
    subprocess.run(["sudo", "chmod", "o+rx", nvram_dir], check=True)
    for vm in vms:
        dest = nvram_dir / f"{vm.name}_VARS.fd"
        subprocess.run(
            ["sudo", "cp", "/usr/share/OVMF/OVMF_VARS_4M.ms.fd", dest], check=True
        )
        subprocess.run(["sudo", "chmod", "666", dest], check=True)
        log.info(f"Created NVRAM file for {vm.name}: {dest}")

    # Make netlaunch config
    hostip = get_host_default_gateway_interface_ip()
    log.info(f"Detected host IP: {hostip}")
    netlaunch_path = make_file(args.netlaunch)
    if len(vms) > 1:
        log.warning(
            "Netlaunch only supports one target, "
            "but multiple VMs were created. "
            f"Only the first one will be used. ({vms[0].name})"
        )
    ConfigHelper().generate_netlaunch(
        vms[0],
        str(hostip),
    ).write(netlaunch_path)
    log.info(f"Wrote netlaunch config to: {netlaunch_path}")


long_desc = """Set up virtual machines, networks and iptables rules for local testing.\n
Machines are defined with arguments in the form "flags[:cpus[:mem[:disk]]]". 
 - flags is a list characters representing optional features to enable for the VM. (Default: '{0}')
 - cpus is the number of virtual CPUs to assign to the VM. (Default: value of '-c')
 - mem  is the amount of memory to assign to the VM in GiB. (Default: value of '-m')
 - disks is a comma separated list of sizes of disks to create for the VM in GiB. (Default: value of '-d')

The flags field is a list of characters representing optional features to enable for the VM.
They are implemented as a bitflags, so they can be combined. Providing multiple flags does a bitwise OR.
Accepted values are:
 - (empty): Use the default flags. (Default: '{0}')
 - b: Base VM. (aka an all zero bitflag) Does not enable any feature. Useful to create a basic VM.
 - t: Enable an emulated TPM 2.0 device in the VM. (requires `swtpm` & `swtpm-tools`)
 
All values can be omitted. Missing values get filled in with their default. 
Examples:
b t bt : :::64 b::: b:4:4:16,32 b:8 b::32 b:::128
""".format(
    str(VMSpecParser.DEFAULT_FLAGS)
)


class CustomFormatter(ArgumentDefaultsHelpFormatter, RawTextHelpFormatter):
    pass


CMD_METADATA = SubCommand(
    "create",
    init,
    run,
    summary="Create libvirt resources",
    description=long_desc,
    formatter_class=CustomFormatter,
)
