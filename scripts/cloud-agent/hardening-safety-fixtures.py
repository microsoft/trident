#!/usr/bin/env python3
"""Bounded libvirt safety probes, invoked only by the Actions harness."""

import hashlib
import importlib.util
import os
import re
import sys
import uuid
import xml.etree.ElementTree as ET
from pathlib import Path

LIBVIRT_ROOT = Path("/var/lib/libvirt")
VAR_ROOT = Path("/var/lib")
FIRMWARE = Path("/usr/share/OVMF/OVMF_CODE_4M.fd")
VARS = Path("/usr/share/OVMF/OVMF_VARS_4M.fd")


def files_evidence(paths):
    result = {}
    for path in paths:
        try:
            info = path.lstat()
        except FileNotFoundError:
            result[str(path)] = None
            continue
        item = {
            "device": info.st_dev,
            "inode": info.st_ino,
            "mode": info.st_mode,
            "uid": info.st_uid,
            "gid": info.st_gid,
            "links": info.st_nlink,
            "mtime_ns": info.st_mtime_ns,
        }
        if path.is_symlink():
            item["symlink"] = os.readlink(path)
        elif path.is_dir():
            item["entries"] = files_evidence(sorted(path.iterdir()))
        else:
            item["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
        result[str(path)] = item
    return result


def configuration_digest(kind, definition):
    root = ET.fromstring(definition)
    if kind == "pool":
        # Libvirt refreshes filesystem accounting without changing configuration.
        for tag in ("capacity", "allocation", "available", "physical"):
            for element in root.findall(tag):
                root.remove(element)
        target = root.find("target")
        if target is not None:
            for element in target.findall("timestamps"):
                target.remove(element)
    normalized = ET.canonicalize(ET.tostring(root, encoding="unicode"), strip_text=True)
    return hashlib.sha256(normalized.encode()).hexdigest()


class Fixture:
    def __init__(self, runner, case):
        self.r = runner
        self.case = case
        self.ns = f"{runner.namespace}-{case}"
        if not re.fullmatch(r"hardening-\d+-\d+-\d+-[a-z-]+", self.ns):
            raise RuntimeError("Unsafe fixture namespace")
        self.original = runner.inventory()
        self.roots = []
        self.resources = []
        self.expected_files = None
        self.r.save(f"{case}-baseline.json", self.original)
        if any(
            name.startswith(self.ns + "-")
            for resources in self.original.values()
            for name in resources
        ):
            raise RuntimeError("Fixture namespace already has libvirt resources")
        if any(
            os.path.lexists(path)
            for path in (
                LIBVIRT_ROOT / self.ns,
                VAR_ROOT / f"{self.ns}-nvram",
            )
        ):
            raise RuntimeError("Fixture canonical storage target already exists")

    def checkpoint(self):
        self.r.save(
            f"{self.case}-manifest.json",
            {
                "namespace": self.ns,
                "resources": self.resources,
                "exclusively_created_paths": files_evidence(self.roots),
            },
        )

    def directory(self, namespace, nvram=False):
        if (
            namespace != self.ns and not namespace.startswith(self.ns + "-")
        ) or not re.fullmatch(r"[a-z0-9-]+", namespace):
            raise RuntimeError("Directory is outside this fixture namespace")
        path = VAR_ROOT / f"{namespace}-nvram" if nvram else LIBVIRT_ROOT / namespace
        # Exclusive mkdir is essential. Never adopt an existing host directory.
        path.mkdir(mode=0o755)
        self.roots.append(path)
        self.checkpoint()
        return path

    def define(self, kind, name, root):
        if not name.startswith(self.ns + "-"):
            raise RuntimeError("Resource is outside this fixture namespace")
        category, command = (
            ("domains", "define") if kind == "domain" else ("pools", "pool-define")
        )
        if name in self.r.inventory()[category]:
            raise RuntimeError(f"Refusing to redefine existing {name}")
        identifier = str(uuid.uuid4())
        ET.SubElement(root, "uuid").text = identifier
        xml = self.r.scratch / f"{name}.xml"
        xml.write_text(ET.tostring(root, encoding="unicode"))
        record = {
            "kind": kind,
            "name": name,
            "uuid": identifier,
            "target": root.findtext("target/path"),
            "nvram": root.findtext("os/nvram"),
        }
        self.resources.append(record)
        self.checkpoint()
        self.r.virsh(command, str(xml))
        self.capture_configuration(record)
        return record

    def capture_configuration(self, record):
        dump = "dumpxml" if record["kind"] == "domain" else "pool-dumpxml"
        definition = self.r.virsh(dump, record["name"])
        record["configuration_sha256"] = configuration_digest(
            record["kind"], definition
        )
        self.checkpoint()

    def pool(self, name, target, active=False):
        root = ET.Element("pool", {"type": "dir"})
        ET.SubElement(root, "name").text = name
        ET.SubElement(ET.SubElement(root, "target"), "path").text = str(target)
        record = self.define("pool", name, root)
        if active:
            self.r.virsh("pool-start", name)
            self.capture_configuration(record)
        return record

    def domain(self, name, disk=None, nvram=None, owned=False):
        root = ET.Element("domain", {"type": "kvm"})
        ET.SubElement(root, "name").text = name
        ET.SubElement(root, "memory", {"unit": "MiB"}).text = "128"
        ET.SubElement(root, "vcpu").text = "1"
        os_xml = ET.SubElement(root, "os")
        ET.SubElement(os_xml, "type", {"arch": "x86_64", "machine": "q35"}).text = "hvm"
        if nvram is not None:
            if not FIRMWARE.is_file():
                raise RuntimeError("Required OVMF firmware is unavailable")
            ET.SubElement(
                os_xml, "loader", {"readonly": "yes", "type": "pflash"}
            ).text = str(FIRMWARE)
            ET.SubElement(os_xml, "nvram").text = str(nvram)
            ET.SubElement(ET.SubElement(root, "features"), "acpi")
        if owned:
            metadata = ET.SubElement(root, "metadata")
            ET.SubElement(
                metadata,
                "{urn:trident:virtdeploy:1}owner",
                {"namespace": self.ns, "kind": "domain"},
            )
        devices = ET.SubElement(root, "devices")
        ET.SubElement(devices, "emulator").text = "/usr/bin/qemu-system-x86_64"
        if disk is not None:
            disk_xml = ET.SubElement(
                devices, "disk", {"type": "file", "device": "disk"}
            )
            ET.SubElement(disk_xml, "driver", {"name": "qemu", "type": "raw"})
            ET.SubElement(disk_xml, "source", {"file": str(disk)})
            ET.SubElement(disk_xml, "target", {"dev": "vda", "bus": "virtio"})
        return self.define("domain", name, root)

    def reject(self, phase, command, reason):
        name = f"{self.case}-{phase}"
        before = {"inventory": self.r.inventory(), "files": files_evidence(self.roots)}
        self.expected_files = before["files"]
        self.r.save(f"{name}-before.json", before)
        try:
            output = self.r.run(
                name, [str(self.r.source / "bin/virtdeploy"), *command], expected=1
            )
        finally:
            after = {
                "inventory": self.r.inventory(),
                "files": files_evidence(self.roots),
            }
            self.r.save(f"{name}-after.json", after)
        if before != after:
            raise RuntimeError(f"{name}: rejection mutated inventory or sentinel files")
        if "ownership preflight" not in output or reason not in output:
            raise RuntimeError(
                f"{name}: expected dependency/recreation rejection was not visible"
            )
        self.r.results[name]["zero_mutations"] = True
        self.r.save("results.json", self.r.results)

    def verified_definition(self, record):
        kind, name = record["kind"], record["name"]
        category = "domains" if kind == "domain" else "pools"
        if name not in self.r.inventory()[category]:
            return
        dump = "dumpxml" if kind == "domain" else "pool-dumpxml"
        definition = self.r.virsh(dump, name)
        parsed = ET.fromstring(definition)
        if (
            parsed.findtext("uuid") != record["uuid"]
            or parsed.findtext("name") != name
            or parsed.findtext("target/path") != record["target"]
            or parsed.findtext("os/nvram") != record["nvram"]
            or configuration_digest(kind, definition)
            != record.get("configuration_sha256")
        ):
            raise RuntimeError(f"Refusing teardown of changed fixture {name}")
        return parsed

    def undefine(self, record):
        kind, name = record["kind"], record["name"]
        parsed = self.verified_definition(record)
        if parsed is None:
            return
        if kind == "domain":
            if self.r.virsh("domstate", name).strip() != "shut off":
                raise RuntimeError(f"Refusing teardown of active fixture {name}")
            args = ["undefine", name]
            if parsed.find("os/nvram") is not None:
                args.append("--keep-nvram")
            self.r.virsh(*args)
        else:
            if re.search(
                r"^State:\s+running\s*$", self.r.virsh("pool-info", name), re.M
            ):
                self.r.virsh("pool-destroy", name)
            self.r.virsh("pool-undefine", name)

    def teardown(self):
        try:
            # Validate every definition before changing even an unchanged sibling.
            for record in reversed(self.resources):
                self.verified_definition(record)
            for record in reversed(self.resources):
                self.undefine(record)
            # Retain changed/partially constructed files for inspection. Never
            # recursively delete a directory or ask virtdeploy to clean a fixture.
            if self.expected_files is not None:
                if files_evidence(self.roots) != self.expected_files:
                    raise RuntimeError("Changed fixture files retained for inspection")
                children = [child for root in self.roots for child in root.iterdir()]
                if any(child.is_dir() and not child.is_symlink() for child in children):
                    raise RuntimeError("Unexpected nested fixture directory")
                for child in children:
                    child.unlink()
                for root in reversed(self.roots):
                    root.rmdir()
            final = self.r.inventory()
            if final != self.original:
                raise RuntimeError(
                    "Fixture teardown did not restore original libvirt inventory"
                )
        finally:
            self.r.save(
                f"{self.case}-teardown.json",
                {
                    "inventory": self.r.inventory(),
                    "retained_paths": files_evidence(self.roots),
                },
            )


def symlink_dependency(fixture):
    root = fixture.directory(fixture.ns)
    aliases = fixture.directory(fixture.ns + "-aliases")
    pool = fixture.ns + "-pool"
    record = fixture.pool(pool, root, active=True)
    sentinel = root / f"{fixture.ns}-sentinel.raw"
    fixture.r.virsh("vol-create-as", pool, sentinel.name, "1MiB", "--format", "raw")
    with sentinel.open("r+b") as output:
        output.write(f"task-owned:{fixture.ns}\n".encode())
        output.flush()
        os.fsync(output.fileno())
    fixture.r.virsh("pool-refresh", pool)
    fixture.capture_configuration(record)
    alias = aliases / "disk-link.raw"
    alias.symlink_to(sentinel)
    retained = fixture.ns + "-retained"
    fixture.domain(retained, disk=alias)
    definition = ET.fromstring(fixture.r.virsh("dumpxml", retained))
    source = definition.find("devices/disk/source")
    if source is None or source.get("file") != str(alias):
        raise RuntimeError(
            "Retained domain did not preserve the symlink disk reference"
        )
    fixture.reject(
        "clean",
        ["clean", "--namespace", fixture.ns],
        f"retained domain {retained} depends on namespace",
    )


def absent_nvram_pool(fixture):
    root = fixture.directory(fixture.ns, nvram=True)
    nvram = root / f"{fixture.ns}-vm-0_VARS.fd"
    with nvram.open("xb") as output:
        output.write(VARS.read_bytes())
    retained = fixture.ns + "-retained-pool"
    fixture.pool(retained, root)
    fixture.domain(fixture.ns + "-vm-0", nvram=nvram, owned=True)
    if fixture.ns + "-nvram-pool" in fixture.r.inventory()["pools"]:
        raise RuntimeError(
            "Absent NVRAM pool fixture unexpectedly has a namespace pool"
        )
    fixture.reject(
        "clean",
        ["clean", "--namespace", fixture.ns],
        f"retained storage pool {retained} overlaps namespace",
    )


def retained_recreation(fixture, network):
    root = fixture.directory(fixture.ns)
    sentinel = root / f"{fixture.ns}-vm-0-volume-0.qcow2"
    with sentinel.open("xb") as output:
        output.write(f"task-owned-retained-volume:{fixture.ns}\n".encode())
    pool = fixture.pool(fixture.ns + "-pool", root)
    command = [
        "create-one",
        "--namespace",
        fixture.ns,
        "--network",
        network,
        "--no-tpm",
        "--no-secure-boot",
        "--cpus",
        "1",
        "--mem",
        "1",
        "--disk",
        "1",
        "--netlaunch",
        str(fixture.r.scratch / "fixture-netlaunch.yaml"),
    ]
    fixture.reject("inactive", command, "unverified files remain in storage directory")
    retained = files_evidence([root])
    fixture.undefine(pool)
    if files_evidence([root]) != retained:
        raise RuntimeError("Pool undefine changed retained fixture files")
    fixture.reject("undefined", command, "unverified files remain in storage directory")


def exercise_fixture(fixture, operation):
    primary = None
    try:
        operation(fixture)
    except BaseException as error:
        primary = error
        raise
    finally:
        try:
            fixture.teardown()
        except BaseException as cleanup_error:
            fixture.r.save(
                f"{fixture.case}-cleanup-failure.json",
                {
                    "primary_failure": (
                        None
                        if primary is None
                        else {
                            "type": type(primary).__name__,
                            "message": str(primary),
                        }
                    ),
                    "cleanup_failure": {
                        "type": type(cleanup_error).__name__,
                        "message": str(cleanup_error),
                    },
                },
            )
            if primary is None:
                raise


def main():
    if (
        not hasattr(os, "geteuid")
        or os.geteuid() != 0
        or os.environ.get("GITHUB_ACTIONS") != "true"
    ):
        raise RuntimeError(
            "Safety fixtures require privileged GitHub Actions execution"
        )
    source, reports, namespace = sys.argv[1:]
    if not re.fullmatch(r"hardening-\d+-\d+-\d+", namespace):
        raise RuntimeError("Unsafe fixture run prefix")
    # Reports contain only redacted logs, resource identities and hashes, and
    # must remain readable by the non-root artifact upload step.
    os.umask(0o022)
    spec = importlib.util.spec_from_file_location(
        "hardening_validation", Path(__file__).with_name("hardening-validation.py")
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    runner = module.Validation(source, reports, namespace=namespace)
    import fcntl

    with module.LOCK.open("rb") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        identity = os.fstat(lock.fileno())
        if (
            module.LOCK.is_symlink()
            or identity.st_nlink != 1
            or not os.path.samestat(identity, module.LOCK.stat())
        ):
            raise RuntimeError("Unsafe fixture host lock")
        completed = []
        try:
            network = runner.run(
                "fixture-subnet",
                [
                    "python3",
                    "scripts/cloud-agent/select_functional_network.py",
                    namespace + "-probe",
                ],
            ).strip()
            import ipaddress

            ipaddress.ip_network(network)
            for case, operation in (
                ("alias", symlink_dependency),
                ("nvram", absent_nvram_pool),
                ("recreation", lambda fixture: retained_recreation(fixture, network)),
            ):
                fixture = Fixture(runner, case)
                exercise_fixture(fixture, operation)
                completed.append(case)
        finally:
            runner.save("results.json", runner.results)
            runner.save(
                "fixture-summary.json",
                {
                    "live_cases_completed": completed,
                    "live_cases_not_completed": [
                        case
                        for case in ("alias", "nvram", "recreation")
                        if case not in completed
                    ],
                    "expected_rejections_verified": sum(
                        result.get("zero_mutations", False)
                        for result in runner.results.values()
                    ),
                    "scope": "real libvirt definitions and raw sentinel volume, no fixture VM boot",
                },
            )
            if not os.path.samestat(identity, module.LOCK.stat()):
                raise RuntimeError("Fixture host lock inode changed")


if __name__ == "__main__":
    main()
