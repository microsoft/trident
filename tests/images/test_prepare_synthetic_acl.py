import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import unittest
from unittest.mock import patch
import uuid


SPEC = importlib.util.spec_from_file_location(
    "prepare_synthetic_acl", Path(__file__).with_name("prepare-synthetic-acl.py")
)
PREPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREPARE)


class SyntheticAclTests(unittest.TestCase):
    def setUp(self):
        self.directory = Path.cwd() / (".synthetic-acl-test-" + uuid.uuid4().hex)
        self.directory.mkdir()
        self.addCleanup(shutil.rmtree, self.directory)

    def cosi(self, *, verity=True, bootloader="systemd-boot"):
        metadata = {
            "version": "1.2",
            "osArch": "x86_64",
            "images": [
                {
                    "mountPoint": "/usr",
                    "verity": (
                        {"roothash": "1234", "image": {"sha384": "unchanged"}}
                        if verity
                        else None
                    ),
                }
            ],
            "bootloader": {
                "type": bootloader,
                "systemdBoot": {
                    "entries": [
                        {"type": "uki-standalone", "path": "/EFI/Linux/vmlinuz.efi"}
                    ]
                },
            },
        }
        path = self.directory / "update.cosi"
        with tarfile.open(path, "w:") as archive:
            for name, data in (
                ("metadata.json", json.dumps(metadata).encode()),
                ("images/usr.raw.zst", b"unaltered partition payload"),
            ):
                member = tarfile.TarInfo(name)
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
        return path

    def test_verity_cmdline_uses_real_slot_addons_and_stable_shared_root(self):
        for root_hash in ("a" * 64, "b" * 64):
            with self.subTest(root_hash=root_hash):
                original = (
                    f"root=UUID=builder-root ro mount.usr=/dev/mapper/usr usrhash={root_hash} "
                    "systemd.verity_usr_data=PARTLABEL=USR-A "
                    "systemd.verity_usr_hash=PARTLABEL=HASH-A "
                    "systemd.verity_usr_options=panic-on-corruption acl.slot=a"
                )
                common, addons = PREPARE.split_verity_cmdline(original)
                self.assertEqual("root=LABEL=ROOT ro mount.usr=/dev/mapper/usr", common)
                for slot, data, hashes in (("a", 2, 3), ("b", 4, 5)):
                    with self.subTest(slot=slot):
                        arguments = dict(
                            token.split("=", 1) for token in addons[slot].split()
                        )
                        self.assertEqual(
                            "PARTUUID=" + PREPARE.ACL_UUIDS[data],
                            arguments["systemd.verity_usr_data"],
                        )
                        self.assertEqual(
                            "PARTUUID=" + PREPARE.ACL_UUIDS[hashes],
                            arguments["systemd.verity_usr_hash"],
                        )
                        self.assertEqual(root_hash, arguments["usrhash"])
                        self.assertEqual(
                            "panic-on-corruption",
                            arguments["systemd.verity_usr_options"],
                        )

    def test_verity_addons_reject_missing_invalid_or_conflicting_hashes(self):
        for cmdline in (
            "root=LABEL=ROOT",
            "usrhash=invalid",
            "usrhash=" + "a" * 64 + " usrhash=" + "b" * 64,
        ):
            with self.subTest(cmdline=cmdline), self.assertRaisesRegex(
                ValueError, "verity root hash"
            ):
                PREPARE.split_verity_cmdline(cmdline)

    def test_stages_static_recipes_and_pins_pipeline_rpm(self):
        for variant in ("standard", "mshv"):
            with self.subTest(variant=variant):
                work = self.directory / variant
                work.mkdir()
                PREPARE.prepare_inputs(
                    work, variant, "trident-pipeline-1.x86_64", set(), self.directory
                )
                for name in (f"{variant}.yaml", "common.yaml", "configure.sh"):
                    self.assertEqual(
                        (PREPARE.HERE / name).read_bytes(), (work / name).read_bytes()
                    )
                self.assertEqual(
                    {"packages": ["trident-pipeline-1.x86_64"]},
                    json.loads((work / "trident-packages.json").read_text()),
                )

    def test_removes_only_packages_present_in_base(self):
        for variant, installed, expected in (
            ("standard", {"kernel", "grub2-efi-binary"}, ["grub2-efi-binary"]),
            ("mshv", {"kernel", "shim"}, ["kernel", "shim"]),
            ("standard", set(), []),
        ):
            with self.subTest(variant=variant, installed=installed):
                work = self.directory / str(len(list(self.directory.iterdir())))
                work.mkdir()
                PREPARE.prepare_inputs(
                    work, variant, "trident-pipeline", installed, self.directory
                )
                self.assertEqual(
                    {"packages": expected},
                    json.loads((work / "remove-packages.json").read_text()),
                )

    def test_generation_input_accepts_only_base_or_update(self):
        generation = self.directory / "generation"
        setup = (PREPARE.HERE / "configure.sh").read_text().split("mkdir -p", 1)[0]
        setup = setup.replace("/usr/lib/synthetic-acl-generation", str(generation))
        for phase in ("base", "update", "", "unexpected"):
            with self.subTest(phase=phase):
                generation.write_text(phase + "\n")
                result = subprocess.run(
                    ["bash", "-s", "--", "standard"],
                    input=setup + 'printf "%s" "$phase"\n',
                    text=True,
                    capture_output=True,
                )
                if phase in ("base", "update"):
                    self.assertEqual(0, result.returncode, result.stderr)
                    self.assertEqual(phase, result.stdout)
                else:
                    self.assertNotEqual(0, result.returncode)

    def test_stages_optional_mshv_license_tree(self):
        licenses = self.directory / "licenses" / "package"
        licenses.mkdir(parents=True)
        (licenses / "EULA").write_text("payload license")
        work = self.directory / "work"
        work.mkdir()
        PREPARE.prepare_inputs(work, "mshv", "trident-pipeline", set(), self.directory)
        self.assertEqual(
            "payload license", (work / "mshv-licenses/package/EULA").read_text()
        )

    def test_ssh_override_precedes_azure_linux_defaults(self):
        etc = self.directory / "etc"
        (etc / "ssh").mkdir(parents=True)
        setup = (
            (PREPARE.HERE / "configure.sh").read_text().split("passwd -d root", 1)[0]
        )
        generation = self.directory / "generation"
        generation.write_text("base\n")
        setup = setup.replace("/usr/lib/synthetic-acl-generation", str(generation))
        setup = setup.replace("/etc/", str(etc) + "/")
        setup = setup.replace("/var/lib/trident", str(self.directory / "trident"))
        setup = setup.replace("/oem", str(self.directory / "oem"))
        include = f"Include {etc}/ssh/sshd_config.d/*.conf"
        for initial in ("PermitRootLogin no\n", include + "\nPermitRootLogin no\n"):
            with self.subTest(initial=initial):
                config = etc / "ssh/sshd_config"
                config.write_text(initial)
                for _ in range(2):
                    subprocess.run(
                        ["bash", "-s", "--", "standard"],
                        input=setup,
                        text=True,
                        check=True,
                        capture_output=True,
                    )
                lines = config.read_text().splitlines()
                self.assertEqual(include, lines[0])
                self.assertEqual(1, lines.count(include))
                self.assertIn("PermitRootLogin no", lines)

    def test_mshv_metadata_upgrade_preserves_payload_and_exact_export(self):
        path = self.cosi()
        PREPARE.publish_metadata(path, self.directory, "mshv")
        with tarfile.open(path) as archive:
            exact = archive.extractfile("metadata.json").read()
            self.assertEqual(exact, (self.directory / "metadata.json").read_bytes())
            metadata = json.loads(exact)
            self.assertEqual("1.3", metadata["version"])
            self.assertEqual(
                "efitreeloader-mshv-systemd-boot", metadata["bootloader"]["type"]
            )
            self.assertEqual(
                "unchanged", metadata["images"][0]["verity"]["image"]["sha384"]
            )
            self.assertEqual(
                b"unaltered partition payload",
                archive.extractfile("images/usr.raw.zst").read(),
            )

    def test_standard_metadata_does_not_claim_mshv(self):
        path = self.cosi()
        PREPARE.publish_metadata(path, self.directory, "standard")
        metadata = json.loads((self.directory / "metadata.json").read_bytes())
        self.assertEqual("1.2", metadata["version"])
        self.assertEqual("systemd-boot", metadata["bootloader"]["type"])

    def test_unprotected_usr_is_rejected_without_replacing_archive(self):
        path = self.cosi(verity=False)
        before = path.read_bytes()
        with self.assertRaisesRegex(ValueError, "verity-protected"):
            PREPARE.publish_metadata(path, self.directory, "mshv")
        self.assertEqual(before, path.read_bytes())

    def test_standard_rejects_mshv_metadata(self):
        path = self.cosi(bootloader="efitreeloader-mshv-systemd-boot")
        with self.assertRaisesRegex(ValueError, "systemd-boot UKI"):
            PREPARE.publish_metadata(path, self.directory, "standard")

    def test_normalization_uses_acl_guids_and_shared_root(self):
        with patch.object(PREPARE, "guestfish") as guestfish:
            PREPARE.normalize_disk(
                self.directory / "base.qcow2",
                self.directory,
                "standard",
                None,
                None,
                {},
            )
        commands = guestfish.call_args.args[1]
        self.assertIn("part-set-gpt-guid /dev/sda 2 " + PREPARE.ACL_UUIDS[2], commands)
        self.assertIn("part-set-gpt-type /dev/sda 4 " + PREPARE.ACL_USR_TYPE, commands)
        self.assertIn("mount /dev/sda7 /", commands)
        self.assertIn(
            "LABEL=EFI-SYSTEM /boot vfat", (self.directory / "fstab").read_text()
        )
        self.assertFalse(any("second.efi" in command for command in commands))

    def test_normalization_removes_builder_mountpoint_after_unmounting_esp(self):
        for variant in ("standard", "mshv"):
            with self.subTest(variant=variant), patch.object(
                PREPARE, "guestfish"
            ) as guestfish:
                PREPARE.normalize_disk(
                    self.directory / "base.qcow2",
                    self.directory,
                    variant,
                    self.directory / "etl.efi",
                    self.directory,
                    {},
                )
                commands = guestfish.call_args.args[1]
                self.assertIn("mount /dev/sda1 /boot/efi", commands[:-3])
                self.assertEqual(
                    ["umount /boot/efi", "rmdir /boot/efi", "umount-all"], commands[-3:]
                )

    def test_mshv_normalization_keeps_second_stage_and_stages_payload(self):
        payload = self.directory / "payload"
        payload.mkdir()
        for name in ("HvLoader.efi", "lxhvloader.dll"):
            (payload / name).write_bytes(b"unit-test fixture")
        (payload / "Windows").mkdir()
        with patch.object(PREPARE, "guestfish") as guestfish:
            PREPARE.normalize_disk(
                self.directory / "base.qcow2",
                self.directory,
                "mshv",
                self.directory / "etl.efi",
                payload,
                {},
            )
        commands = guestfish.call_args.args[1]
        self.assertIn(
            "cp /boot/efi/EFI/BOOT/grubx64.efi /boot/efi/EFI/BOOT/second.efi", commands
        )
        self.assertTrue(
            any(
                "upload " in command and command.endswith("/EFI/BOOT/grubx64.efi")
                for command in commands
            )
        )
        self.assertTrue(
            any("copy-in " in command and "/Windows" in command for command in commands)
        )


if __name__ == "__main__":
    unittest.main()
