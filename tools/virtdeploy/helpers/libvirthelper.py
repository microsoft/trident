import ipaddress
import tempfile
import jinja2
import libvirt
import logging
import random
import xml.etree.ElementTree as ET

from os import path as pt
from pathlib import Path
from typing import Any, Dict, List, NamedTuple, Optional

from .cloudinit import build_cloud_init_iso
from .vmmetadata import CloudInitConfig, ConfigFlags, VirtualMachineTemplate

log = logging.getLogger(__name__)


def generate_mac():
    return "52:54:00:{:02x}:{:02x}:{:02x}".format(
        random.randint(00, 255), random.randint(00, 255), random.randint(00, 255)
    )


def get_pool_location(pool: libvirt.virStoragePool):
    tree = ET.fromstring(pool.XMLDesc())
    return tree.find("./target/path").text


class VirtualMachine:
    SERIALIZE_KEY_NAME = "name"
    SERIALIZE_KEY_IP = "ip"
    SERIALIZE_KEY_UUID = "uuid"
    SERIALIZE_KEY_ROLE = "role"
    SERIALIZE_KEY_CPUS = "cpus"

    def __init__(
        self,
        template: VirtualMachineTemplate,
        ip: ipaddress.IPv4Address,
        generate_mac_address=True,
    ) -> None:
        if template == None:
            template = VirtualMachineTemplate()
        self.name = template.name
        self.ip = ip
        self.mac = None
        if generate_mac_address:
            self.mac = generate_mac()
        self.cpus = template.cpus
        self.mem = template.mem
        self.disks = template.disks
        self._domain: libvirt.virDomain = None
        self.UUIDString: str = None
        self.bmc_ip: str = None
        self.flags: ConfigFlags = template.flags
        self.os_disk: Optional[Path] = template.os_disk
        self.cloud_init: Optional[CloudInitConfig] = template.cloud_init

    def set_domain(self, domain: libvirt.virDomain) -> None:
        self._domain = domain
        self.UUIDString = domain.UUIDString()

    def export_data(self) -> Dict[str, str]:
        return {
            VirtualMachine.SERIALIZE_KEY_NAME: self.name,
            VirtualMachine.SERIALIZE_KEY_IP: str(self.ip),
            VirtualMachine.SERIALIZE_KEY_UUID: self.UUIDString,
            VirtualMachine.SERIALIZE_KEY_CPUS: self.cpus,
        }

    @staticmethod
    def import_data(data: Dict[str, str]) -> "VirtualMachine":
        vm = VirtualMachine(
            None,
            ipaddress.ip_address(data[VirtualMachine.SERIALIZE_KEY_IP]),
            generate_mac_address=False,
        )
        vm.cpus = data[VirtualMachine.SERIALIZE_KEY_CPUS]
        vm.name = data[VirtualMachine.SERIALIZE_KEY_NAME]
        vm.UUIDString = data[VirtualMachine.SERIALIZE_KEY_UUID]
        return vm


class Network:
    def __init__(
        self,
        name: str,
        address: str,
    ) -> None:
        self.name = name
        try:
            self.config = ipaddress.ip_network(address)
        except ValueError as err:
            log.critical(f"Not a valid network! {address}, expected: X.X.X.X/Y")
            exit(1)

        self.hosts = list(self.config.hosts())
        if len(self.hosts) < 16:
            log.error(f"Provided network {self.config} is too small!")
        self.address = self.hosts[0]
        self.netmask = self.config.netmask
        self.dhcp_start = self.hosts[1]
        self.dhcp_end = self.hosts[-1]
        self.leased = 0

    def lease(self) -> ipaddress.IPv4Address:
        # Skip the
        ip = self.hosts[self.leased + 1]
        self.leased += 1
        return ip

    def check_hosts(self, vms: List[VirtualMachine]) -> bool:
        for vm in vms:
            if vm.ip not in self.config:
                log.error(f"Virtual Machine '{vm.name}' IP not inside network!")
                return False
        return True


class LibvirtHelper:
    DEFAULT_POOL_NAME = "default"
    DEFAULT_POOL_LOCATION = "/var/lib/libvirt/images"
    DEFAULT_DISK = "/dev/sda"

    def __init__(
        self,
        prefix: str,
        networkstr: str,
        vm_templates: List[VirtualMachineTemplate],
        network_interface: str,
        dryrun: bool = False,
    ) -> None:
        self.prefix = prefix
        self.net = Network(f"{prefix}-network", networkstr)
        self.vms = self._generate_vms_metadata(vm_templates)
        self.network_interface = network_interface
        self.dryrun = dryrun
        self.env = jinja2.Environment(
            loader=jinja2.PackageLoader("virtdeploy"),
            autoescape=jinja2.select_autoescape(),
        )

        def libvirt_callback(userdata, err):
            log.debug(err)

        libvirt.registerErrorHandler(f=libvirt_callback, ctx=None)

        try:
            log.debug("Trying to connect...")
            self.conn = libvirt.open(None)
            log.info("Libvirt connection: OK")
        except libvirt.libvirtError as ex:
            log.critical(
                f"Failed to open connection to the hypervisor: {ex}\nIs your user in the libvirt group?"
            )
            exit(1)

    def __del__(self) -> None:
        if hasattr(self, "conn") and self.conn is not None and self.conn.isAlive():
            self.conn.close()

    def construct(self) -> List[VirtualMachine]:
        # Create the network to connect VMs to
        self._setup_network()

        # Setu-up the default pool, required by sushy
        default_pool = self._setup_pool(
            LibvirtHelper.DEFAULT_POOL_NAME, LibvirtHelper.DEFAULT_POOL_LOCATION
        )

        # Set up pool for our own use
        pool_name = f"{self.prefix}-pool"
        pool = self._setup_pool(
            pool_name,
            pt.join(LibvirtHelper.DEFAULT_POOL_LOCATION, pool_name),
            delete_old=True,
        )

        # Setup domains and return
        return self._setup_vms(pool)

    def _delete_network_by_name(self, net_name: str) -> None:
        try:
            net = self.conn.networkLookupByName(net_name)
            log.info(f'Network "{net_name}" found, deleting...')
            self._delete_network(net)
        except libvirt.libvirtError:
            pass

    def _delete_network(self, net: libvirt.virNetwork) -> None:
        try:
            if net.isActive():
                net.destroy()
            net.undefine()
        except libvirt.libvirtError as ex:
            log.error(f'Failed to delete network: "{net.name}":\n{ex}')

    def _delete_pool(self, pool: libvirt.virStoragePool) -> None:
        try:
            for vol in pool.listAllVolumes():
                self._delete_volume(vol)
            if pool.isActive():
                pool.destroy()
            pool.delete()
            pool.undefine()
        except libvirt.libvirtError as ex:
            log.error(f'Failed to delete pool: "{pool.name()}":\n{ex}')

    def _delete_volume_by_name(
        self, pool: libvirt.virStoragePool, vol_name: str
    ) -> None:
        try:
            vol = pool.storageVolLookupByName(vol_name)
            log.warning(f'Pre-existing volume "{vol_name}" found. Deleting!')
            self._delete_volume(pool, vol)
        except libvirt.libvirtError:
            pass

    def _delete_volume(self, vol: libvirt.virStorageVol) -> None:
        try:
            vol.delete()
        except libvirt.libvirtError as ex:
            log.error(f'Failed to delete volume: "{vol.name}":\n{ex}')

    def _delete_domain_by_name(self, dom_name: str):
        try:
            dom = self.conn.lookupByName(dom_name)
            log.warning(f'Pre-existing VM "{dom_name}" found. Deleting!')
            self._delete_domain(dom)
        except libvirt.libvirtError:
            pass

    def _delete_domain(self, dom: libvirt.virDomain) -> None:
        try:
            if dom.isActive():
                dom.destroy()
            dom.undefineFlags(flags=libvirt.VIR_DOMAIN_UNDEFINE_NVRAM)
        except libvirt.libvirtError as ex:
            log.error(f'Failed to delete domain: "{dom.name}":\n{ex}')

    def _generate_vms_metadata(
        self, templates: List[VirtualMachineTemplate]
    ) -> List[VirtualMachine]:
        vms = []
        for i, template in enumerate(templates):
            vm = VirtualMachine(template, self.net.lease())
            vms.append(vm)
        return vms

    def _setup_network(self) -> None:
        self._delete_network_by_name(self.net.name)
        template = self.env.get_template("network.xml")
        xml = template.render(
            name=self.net.name,
            interface=self.network_interface,
            address=str(self.net.address),
            mask=str(self.net.netmask),
            dhcp_start=str(self.net.dhcp_start),
            dhcp_end=str(self.net.dhcp_end),
            hosts=self.vms,
        )

        virtnet = self.conn.networkDefineXML(xml)
        if not virtnet.isActive():
            virtnet.create()
        virtnet.setAutostart(1)

    def _setup_pool(
        self, name: str, path: str, delete_old=False
    ) -> libvirt.virStoragePool:
        pool = None

        try:
            pool = self.conn.storagePoolLookupByName(name)
            if pool != None and delete_old:
                log.warning(f'Pre-existing pool "{name}" found. Deleting!')
                pool = self._delete_pool(pool)
            elif pool != None:
                log.info(f'Using pre-existing pool "{name}"')
        except libvirt.libvirtError:
            pass

        # Create pool if needed
        if pool == None:
            log.warning("Pool not found, creating!")
            # make_dir(path)
            template = self.env.get_template("pool.xml")
            xml = template.render(name=name, path=path)
            pool = self.conn.storagePoolDefineXML(xml)
            pool.build()

        if not pool.isActive():
            pool.create()

        pool.setAutostart(1)
        return pool

    def _create_volume(
        self, pool: libvirt.virStoragePool, volname: str, capacity: int
    ) -> libvirt.virStorageVol:
        """Create a new volume in the specified pool."""
        pool_path = get_pool_location(pool)
        log.info(
            f"Creating volume: {volname} in pool: {pool.name()} at path: {pool_path}"
        )
        vol_template = self.env.get_template("volume.xml")
        vol_xml = vol_template.render(name=volname, capacity=capacity, path=pool_path)
        return pool.createXML(vol_xml)

    def _upload_volume(self, volume: libvirt.virStorageVol, base: Path) -> None:
        """Upload a base image to the specified volume."""
        log.info(f"Uploading base image from '{base}' to '{volume.path()}'")
        stream = self.conn.newStream(0)
        volume.upload(stream, 0, 0)
        with base.open(mode="rb") as f:
            stream.sendAll(lambda stream, nbytes, opaque: opaque.read(nbytes), f)
            stream.finish()

    def _setup_volume(
        self,
        volname: str,
        pool: libvirt.virStoragePool,
        capacity: int,
        base: Optional[Path] = None,
    ) -> libvirt.virStorageVol:
        # Locate and destroy any old volumes
        self._delete_volume_by_name(pool, volname)

        volume = self._create_volume(pool, volname, capacity)

        # If a base is provided, upload it to the volume.
        if base is not None:
            self._upload_volume(volume, base)
        else:
            log.info(f"Creating empty volume: {volume.path()}")
        return volume

    def _setup_vms(self, pool: libvirt.virStoragePool) -> List[VirtualMachine]:
        for vm in self.vms:
            # Locate and destroy any old VMs
            self._delete_domain_by_name(vm.name)

            # Setup volume for the VM
            VolumeParameters = NamedTuple(
                "VolumeParameters", [("source", str), ("dev", str)]
            )
            volumes: List[VolumeParameters] = []

            for index, disk in enumerate(vm.disks):
                volname = f"{vm.name}-{index}-volume.qcow2"
                source = self._setup_volume(
                    volname, pool, disk, vm.os_disk if index == 0 else None
                ).path()
                dev = f"sd{chr(ord('a') + index)}"
                volume = VolumeParameters(source, dev)
                volumes.append(volume)

            # Create a list of cdrom sources, starting with None, for an empty
            # cdrom drive.
            cdrom_paths: Optional[Path] = [None]
            if vm.cloud_init is not None:
                log.info(
                    f"Using cloud-init userdata from user:'{vm.cloud_init.userdata}' meta:'{vm.cloud_init.metadata}' for VM '{vm.name}'"
                )
                ci_cdrom = self._setup_cloud_init_volume(
                    pool,
                    f"{vm.name}-cloud-init.iso",
                    vm.cloud_init,
                )
                cdrom_paths.append(ci_cdrom.path())

            # Convert list of cdroms into VolumeParameters with source and dev
            # in the form sr0, sr1, etc.
            cdroms: List[VolumeParameters] = [
                VolumeParameters(
                    source=cdrom,
                    dev=f"sd{chr(ord('z') - index)}",
                )
                for index, cdrom in enumerate(cdrom_paths)
            ]

            # Set up VM itself
            template = self.env.get_template("vm.xml")
            xml = template.render(
                name=vm.name,
                cpus=vm.cpus,
                mem=vm.mem,
                volumes=volumes,
                mac=vm.mac,
                network=self.net.name,
                cdroms=cdroms,
                enable_tpm=ConfigFlags.EMULATED_TPM in vm.flags,
            )

            log.debug(f"VM XML for {vm.name}:\n{xml}")

            vm.set_domain(self.conn.defineXMLFlags(xml, libvirt.VIR_DOMAIN_XML_SECURE))

        return self.vms

    def _is_in_namespace(self, resource: Any) -> bool:
        if hasattr(resource, "name") and isinstance(resource.name(), str):
            return resource.name().startswith(self.prefix)
        else:
            raise NotImplementedError(
                f'Resource {resource} does not have attribute "name"!'
            )

    def _setup_cloud_init_volume(
        self,
        pool: libvirt.virStoragePool,
        name: str,
        config: CloudInitConfig,
    ) -> libvirt.virStorageVol:
        volume = self._create_volume(pool, name, 1)

        with tempfile.NamedTemporaryFile(
            mode="w+b", suffix=".iso", delete=False
        ) as temp_iso:
            iso_path = Path(temp_iso.name)
            build_cloud_init_iso(iso_path, config)
            self._upload_volume(volume, iso_path)

        return volume

    def clean(self) -> None:
        def find_and_delete(items, delete_func):
            for item in items:
                if not self._is_in_namespace(item):
                    continue
                log.info(f"Deleting: {item.name()}")
                delete_func(item)

        find_and_delete(self.conn.listAllDomains(), self._delete_domain)
        find_and_delete(self.conn.listAllStoragePools(), self._delete_pool)
        find_and_delete(self.conn.listAllNetworks(), self._delete_network)
