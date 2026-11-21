import copy
import importlib.util
import json
import os
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest.mock import Mock, call, patch

SPEC = importlib.util.spec_from_file_location(
    "hardening_safety_fixtures",
    Path(__file__).resolve().parents[1] / "hardening-safety-fixtures.py",
)
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)


class SafetyFixtureTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.libvirt = self.root / "libvirt"
        self.var = self.root / "var"
        self.libvirt.mkdir()
        self.var.mkdir()
        paths = patch.multiple(fixtures, LIBVIRT_ROOT=self.libvirt, VAR_ROOT=self.var)
        paths.start()
        self.addCleanup(paths.stop)
        self.r = Mock()
        self.r.namespace = "hardening-1-2-123"
        self.r.source = self.root
        self.r.scratch = self.root / "scratch"
        self.r.scratch.mkdir()
        self.r.results = {}
        self.inventory = {"domains": {}, "networks": {}, "pools": {}}
        self.r.inventory.side_effect = lambda: copy.deepcopy(self.inventory)
        self.r.virsh.side_effect = AssertionError("Unmocked libvirt operation")
        self.r.save.side_effect = lambda name, value: (self.root / name).write_text(
            json.dumps(value)
        )
        self.f = fixtures.Fixture(self.r, "alias")

    def sentinel(self):
        root = self.f.directory(self.f.ns)
        file = root / "sentinel"
        file.write_text("task-owned sentinel")
        return root, file

    def rejection(self, output="ownership preflight: expected dependency"):
        self.r.run.side_effect = None
        self.r.run.return_value = output
        self.r.results["alias-clean"] = {"exit": 1, "expected_exit": 1}

    def test_existing_directory_and_prefix_collision_are_never_adopted(self):
        root, file = self.sentinel()
        with self.assertRaises(FileExistsError):
            self.f.directory(self.f.ns)
        with self.assertRaisesRegex(RuntimeError, "outside"):
            self.f.directory(self.f.ns + "collision")
        self.assertEqual(self.f.roots, [root])
        self.assertEqual(file.read_text(), "task-owned sentinel")

    def test_namespace_with_preexisting_resource_is_rejected_before_creation(self):
        self.inventory["domains"][self.f.ns + "-vm-9"] = {}
        with self.assertRaisesRegex(RuntimeError, "already has libvirt resources"):
            fixtures.Fixture(self.r, "alias")
        self.r.virsh.assert_not_called()
        self.assertEqual(list(self.libvirt.iterdir()), [])

    def test_unused_canonical_nvram_target_must_also_be_absent(self):
        root = self.var / f"{self.f.ns}-nvram"
        root.mkdir()
        (root / "unrelated").write_text("preserve")
        with self.assertRaisesRegex(RuntimeError, "target already exists"):
            fixtures.Fixture(self.r, "alias")
        self.r.virsh.assert_not_called()
        self.assertEqual((root / "unrelated").read_text(), "preserve")

    def test_existing_resource_definition_is_never_replaced(self):
        name = self.f.ns + "-pool"
        self.inventory["pools"][name] = {}
        with self.assertRaisesRegex(RuntimeError, "Refusing to redefine"):
            self.f.pool(name, self.libvirt / self.f.ns)
        self.r.virsh.assert_not_called()

    def test_expected_rejection_saves_unchanged_files_and_inventory(self):
        self.sentinel()
        self.rejection()
        self.f.reject(
            "clean", ["clean", "--namespace", self.f.ns], "expected dependency"
        )
        before = json.loads((self.root / "alias-clean-before.json").read_text())
        after = json.loads((self.root / "alias-clean-after.json").read_text())
        self.assertEqual(before, after)
        self.assertTrue(self.r.results["alias-clean"]["zero_mutations"])
        self.assertEqual(self.r.run.call_args.kwargs["expected"], 1)

    def test_wrong_rejection_reason_does_not_pass(self):
        self.sentinel()
        self.rejection("permission denied")
        with self.assertRaisesRegex(RuntimeError, "not visible"):
            self.f.reject("clean", ["clean"], "expected dependency")
        self.assertNotIn("zero_mutations", self.r.results["alias-clean"])

    def test_inventory_mutation_cannot_pass_even_when_sentinel_is_intact(self):
        self.sentinel()
        self.rejection()

        def mutate(*args, **kwargs):
            self.inventory["domains"]["unexpected-domain"] = {}
            return "ownership preflight: expected dependency"

        self.r.run.side_effect = mutate
        with self.assertRaisesRegex(RuntimeError, "mutated inventory or sentinel"):
            self.f.reject("clean", ["clean"], "expected dependency")
        self.assertNotIn("zero_mutations", self.r.results["alias-clean"])

    def test_mutated_sentinel_is_detected_and_retained_not_deleted(self):
        root, file = self.sentinel()
        self.rejection()

        def mutate(*args, **kwargs):
            file.write_text("unexpected mutation")
            return "ownership preflight: expected dependency"

        self.r.run.side_effect = mutate
        with self.assertRaisesRegex(RuntimeError, "mutated inventory or sentinel"):
            self.f.reject("clean", ["clean"], "expected dependency")
        with self.assertRaisesRegex(RuntimeError, "Changed fixture files retained"):
            self.f.teardown()
        self.assertTrue(root.is_dir())
        self.assertEqual(file.read_text(), "unexpected mutation")
        self.assertTrue((self.root / "alias-teardown.json").is_file())

    def test_unexpected_command_exit_still_saves_after_evidence(self):
        self.sentinel()
        self.r.run.side_effect = RuntimeError("unexpected exit 0")
        with self.assertRaisesRegex(RuntimeError, "unexpected exit 0"):
            self.f.reject("clean", ["clean"], "expected dependency")
        self.assertTrue((self.root / "alias-clean-after.json").is_file())

    def record(self, kind, nvram=None, target=None):
        name = self.f.ns + ("-vm-0" if kind == "domain" else "-pool")
        self.inventory["domains" if kind == "domain" else "pools"][name] = {}
        record = {
            "kind": kind,
            "name": name,
            "uuid": "fixture-uuid",
            "nvram": nvram,
            "target": target,
        }
        root = ET.Element(kind)
        ET.SubElement(root, "name").text = name
        ET.SubElement(root, "uuid").text = record["uuid"]
        if target is not None:
            ET.SubElement(ET.SubElement(root, "target"), "path").text = target
        if nvram is not None:
            ET.SubElement(ET.SubElement(root, "os"), "nvram").text = nvram
        record["configuration_sha256"] = fixtures.configuration_digest(
            kind, ET.tostring(root, encoding="unicode")
        )
        return record

    def test_creation_captures_libvirt_normalized_full_configuration(self):
        stored = None

        def virsh(command, argument):
            nonlocal stored
            if command == "define":
                root = ET.parse(argument).getroot()
                ET.SubElement(root, "description").text = (
                    "libvirt-normalized definition"
                )
                stored = ET.tostring(root, encoding="unicode")
                return ""
            self.assertEqual(command, "dumpxml")
            return stored

        self.r.virsh.side_effect = virsh
        record = self.f.domain(self.f.ns + "-retained", disk="/task-owned/disk")
        self.assertEqual(
            record["configuration_sha256"],
            fixtures.configuration_digest("domain", stored),
        )
        manifest = json.loads((self.root / "alias-manifest.json").read_text())
        self.assertEqual(manifest["resources"][0], record)

    def test_same_uuid_changed_disk_is_retained_and_primary_failure_survives(self):
        _, sentinel = self.sentinel()
        record = self.record("domain")
        original = (
            f"<domain><name>{record['name']}</name><uuid>fixture-uuid</uuid>"
            "<devices><disk><source file='/task-owned/disk'/></disk></devices></domain>"
        )
        record["configuration_sha256"] = fixtures.configuration_digest(
            "domain", original
        )
        self.f.resources.append(record)
        self.r.virsh.side_effect = None
        self.r.virsh.return_value = original.replace(
            "/task-owned/disk", "/changed/disk"
        )
        self.rejection()

        def mutate(*args, **kwargs):
            self.inventory["domains"][record["name"]] = {"xml_sha256": "changed"}
            return "ownership preflight: expected dependency"

        self.r.run.side_effect = mutate
        with self.assertRaisesRegex(RuntimeError, "rejection mutated inventory"):
            fixtures.exercise_fixture(
                self.f,
                lambda fixture: fixture.reject(
                    "clean", ["clean"], "expected dependency"
                ),
            )
        self.assertEqual(self.r.virsh.call_args_list, [call("dumpxml", record["name"])])
        self.assertIn(record["name"], self.inventory["domains"])
        self.assertTrue(sentinel.is_file())
        report = json.loads((self.root / "alias-cleanup-failure.json").read_text())
        self.assertIn(
            "rejection mutated inventory", report["primary_failure"]["message"]
        )
        self.assertIn("Refusing teardown", report["cleanup_failure"]["message"])

    def test_same_uuid_changed_pool_preserves_unchanged_sibling_too(self):
        _, sentinel = self.sentinel()
        pool = self.record("pool", target=str(sentinel.parent))
        domain = self.record("domain")
        pool_xml = (
            f"<pool><name>{pool['name']}</name><uuid>fixture-uuid</uuid><target>"
            f"<path>{pool['target']}</path><permissions><mode>0755</mode></permissions>"
            "</target></pool>"
        )
        pool["configuration_sha256"] = fixtures.configuration_digest("pool", pool_xml)
        domain_xml = (
            f"<domain><name>{domain['name']}</name><uuid>fixture-uuid</uuid></domain>"
        )
        self.f.resources = [pool, domain]

        def read_definition(command, name):
            if command == "dumpxml":
                self.assertEqual(name, domain["name"])
                return domain_xml
            self.assertEqual((command, name), ("pool-dumpxml", pool["name"]))
            return pool_xml.replace("0755", "0700")

        self.r.virsh.side_effect = read_definition
        self.rejection()

        def mutate(*args, **kwargs):
            self.inventory["pools"][pool["name"]] = {"xml_sha256": "changed"}
            return "ownership preflight: expected dependency"

        self.r.run.side_effect = mutate
        with self.assertRaisesRegex(RuntimeError, "rejection mutated inventory"):
            fixtures.exercise_fixture(
                self.f,
                lambda fixture: fixture.reject(
                    "clean", ["clean"], "expected dependency"
                ),
            )
        self.assertEqual(
            self.r.virsh.call_args_list,
            [
                call("dumpxml", domain["name"]),
                call("pool-dumpxml", pool["name"]),
            ],
        )
        self.assertIn(pool["name"], self.inventory["pools"])
        self.assertIn(domain["name"], self.inventory["domains"])
        self.assertTrue(sentinel.is_file())

    def test_configuration_normalization_ignores_only_pool_counters_and_formatting(
        self,
    ):
        first = (
            "<pool type='dir'><name>fixture</name><capacity>100</capacity>"
            "<allocation>10</allocation><available>90</available>"
            "<target><path>/task-owned</path><permissions><mode>0755</mode>"
            "</permissions></target></pool>"
        )
        second = first.replace(">100<", ">200<").replace(">10<", ">20<")
        second = second.replace("><", ">\n  <")
        self.assertEqual(
            fixtures.configuration_digest("pool", first),
            fixtures.configuration_digest("pool", second),
        )
        self.assertNotEqual(
            fixtures.configuration_digest("pool", first),
            fixtures.configuration_digest("pool", second.replace("0755", "0700")),
        )

    def test_cleanup_failure_does_not_replace_primary_exception(self):
        primary = RuntimeError("primary safety failure")
        with patch.object(
            self.f, "teardown", side_effect=ValueError("cleanup failure")
        ):
            with self.assertRaises(RuntimeError) as caught:
                fixtures.exercise_fixture(self.f, Mock(side_effect=primary))
        self.assertIs(caught.exception, primary)
        report = json.loads((self.root / "alias-cleanup-failure.json").read_text())
        self.assertEqual(report["cleanup_failure"]["type"], "ValueError")

    def test_cleanup_failure_without_primary_is_raised_and_reported(self):
        cleanup = RuntimeError("cleanup failure")
        with patch.object(self.f, "teardown", side_effect=cleanup):
            with self.assertRaises(RuntimeError) as caught:
                fixtures.exercise_fixture(self.f, Mock())
        self.assertIs(caught.exception, cleanup)
        report = json.loads((self.root / "alias-cleanup-failure.json").read_text())
        self.assertIsNone(report["primary_failure"])

    def test_uuid_mismatch_refuses_teardown_without_mutation(self):
        record = self.record("domain")
        self.r.virsh.side_effect = None
        self.r.virsh.return_value = (
            f"<domain><name>{record['name']}</name><uuid>other-uuid</uuid></domain>"
        )
        with self.assertRaisesRegex(RuntimeError, "Refusing teardown"):
            self.f.undefine(record)
        self.assertEqual(self.r.virsh.call_args_list, [call("dumpxml", record["name"])])

    def test_missing_configuration_baseline_refuses_teardown_without_mutation(self):
        record = self.record("domain")
        del record["configuration_sha256"]
        self.r.virsh.side_effect = None
        self.r.virsh.return_value = (
            f"<domain><name>{record['name']}</name><uuid>fixture-uuid</uuid></domain>"
        )
        with self.assertRaisesRegex(RuntimeError, "Refusing teardown"):
            self.f.undefine(record)
        self.assertEqual(self.r.virsh.call_args_list, [call("dumpxml", record["name"])])

    def test_domain_teardown_keeps_nvram_instead_of_unlinking_it(self):
        nvram = str(self.var / "owned-vars.fd")
        record = self.record("domain", nvram=nvram)
        self.r.virsh.side_effect = [
            f"<domain><name>{record['name']}</name><uuid>fixture-uuid</uuid>"
            f"<os><nvram>{nvram}</nvram></os></domain>",
            "shut off\n",
            "",
        ]
        self.f.undefine(record)
        self.assertEqual(
            self.r.virsh.call_args_list[-1],
            call("undefine", record["name"], "--keep-nvram"),
        )

    def test_active_domain_is_not_destroyed_by_fixture_teardown(self):
        record = self.record("domain")
        self.r.virsh.side_effect = [
            f"<domain><name>{record['name']}</name><uuid>fixture-uuid</uuid></domain>",
            "running\n",
        ]
        with self.assertRaisesRegex(RuntimeError, "active fixture"):
            self.f.undefine(record)
        self.assertEqual(len(self.r.virsh.call_args_list), 2)

    def test_changed_pool_target_is_never_stopped_or_undefined(self):
        record = self.record("pool", target=str(self.libvirt / self.f.ns))
        self.r.virsh.side_effect = [
            f"<pool><name>{record['name']}</name><uuid>fixture-uuid</uuid>"
            "<target><path>/unrelated/existing</path></target></pool>",
        ]
        with self.assertRaisesRegex(RuntimeError, "changed fixture"):
            self.f.undefine(record)
        self.assertEqual(len(self.r.virsh.call_args_list), 1)

    def test_pool_teardown_stops_only_exact_pool_without_volume_deletion(self):
        target = str(self.libvirt / self.f.ns)
        record = self.record("pool", target=target)
        self.r.virsh.side_effect = [
            f"<pool><name>{record['name']}</name><uuid>fixture-uuid</uuid>"
            "<capacity>200</capacity><allocation>20</allocation><available>180</available>"
            f"<target><path>{target}</path></target></pool>",
            "State:          running\n",
            "",
            "",
        ]
        self.f.undefine(record)
        self.assertEqual(
            self.r.virsh.call_args_list,
            [
                call("pool-dumpxml", record["name"]),
                call("pool-info", record["name"]),
                call("pool-destroy", record["name"]),
                call("pool-undefine", record["name"]),
            ],
        )

    def test_file_teardown_removes_only_unchanged_exclusively_created_root(self):
        root, _ = self.sentinel()
        neighbor = self.libvirt / "unrelated"
        neighbor.mkdir()
        (neighbor / "keep").write_text("unrelated")
        self.f.expected_files = fixtures.files_evidence(self.f.roots)
        self.f.teardown()
        self.assertFalse(root.exists())
        self.assertEqual((neighbor / "keep").read_text(), "unrelated")
        self.r.virsh.assert_not_called()

    def test_recreation_tests_both_inactive_and_undefined_without_start(self):
        root = self.libvirt / self.f.ns
        root.mkdir()
        fixture = Mock()
        fixture.ns = self.f.ns
        fixture.directory.return_value = root
        fixture.r = self.r
        fixtures.retained_recreation(fixture, "192.168.123.0/24")
        self.assertEqual(
            [args.args[0] for args in fixture.reject.call_args_list],
            ["inactive", "undefined"],
        )
        fixture.undefine.assert_called_once_with(fixture.pool.return_value)
        command = fixture.reject.call_args.args[1]
        self.assertNotIn("--start", command)
        self.assertNotIn("--os-disk", command)
        self.assertIn(str(self.r.scratch / "fixture-netlaunch.yaml"), command)

    def test_alias_fixture_references_real_canonical_volume_via_symlink(self):
        root = self.libvirt / self.f.ns
        aliases = self.libvirt / (self.f.ns + "-aliases")
        root.mkdir()
        aliases.mkdir()
        fixture = Mock()
        fixture.ns = self.f.ns
        fixture.r = self.r
        fixture.directory.side_effect = [root, aliases]

        def create_volume(*args):
            if args[0] == "vol-create-as":
                (root / args[2]).write_bytes(b"\0" * 1024)
                return ""
            if args[0] == "pool-refresh":
                self.assertEqual(args[1], fixture.ns + "-pool")
                return ""
            self.assertEqual(args, ("dumpxml", fixture.ns + "-retained"))
            return (
                "<domain><devices><disk><source "
                f"file='{aliases / 'disk-link.raw'}'/></disk></devices></domain>"
            )

        self.r.virsh.side_effect = create_volume
        with patch.object(Path, "symlink_to") as symlink:
            fixtures.symlink_dependency(fixture)
        symlink.assert_called_once_with(root / f"{fixture.ns}-sentinel.raw")
        fixture.domain.assert_called_once_with(
            fixture.ns + "-retained", disk=aliases / "disk-link.raw"
        )
        fixture.reject.assert_called_once()

    def test_owned_domain_xml_has_canonical_nvram_and_ownership_marker(self):
        firmware = self.root / "firmware.fd"
        firmware.write_text("synthetic firmware")
        nvram = self.var / f"{self.f.ns}-nvram" / f"{self.f.ns}-vm-0_VARS.fd"
        with patch.object(fixtures, "FIRMWARE", firmware), patch.object(
            self.f, "define"
        ) as define:
            self.f.domain(self.f.ns + "-vm-0", nvram=nvram, owned=True)
        root = define.call_args.args[2]
        self.assertEqual(root.findtext("os/nvram"), str(nvram))
        owner = root.find("metadata/{urn:trident:virtdeploy:1}owner")
        self.assertEqual(owner.attrib, {"namespace": self.f.ns, "kind": "domain"})

    def test_absent_nvram_pool_case_uses_retained_other_pool_at_actual_target(self):
        root = self.var / f"{self.f.ns}-nvram"
        root.mkdir()
        template = self.root / "template.fd"
        template.write_bytes(b"synthetic nvram")
        fixture = Mock()
        fixture.ns = self.f.ns
        fixture.r = self.r
        fixture.directory.return_value = root
        with patch.object(fixtures, "VARS", template):
            fixtures.absent_nvram_pool(fixture)
        fixture.pool.assert_called_once_with(fixture.ns + "-retained-pool", root)
        nvram = root / f"{fixture.ns}-vm-0_VARS.fd"
        fixture.domain.assert_called_once_with(
            fixture.ns + "-vm-0", nvram=nvram, owned=True
        )
        self.assertEqual(nvram.read_bytes(), template.read_bytes())
        self.assertIn("retained storage pool", fixture.reject.call_args.args[2])

    def test_live_entrypoint_refuses_non_actions_or_unprivileged_execution(self):
        with patch.dict(os.environ, GITHUB_ACTIONS="false"):
            with self.assertRaisesRegex(RuntimeError, "privileged GitHub Actions"):
                fixtures.main()
        with patch.dict(os.environ, GITHUB_ACTIONS="true"), patch.object(
            os, "geteuid", return_value=1000, create=True
        ):
            with self.assertRaisesRegex(RuntimeError, "privileged GitHub Actions"):
                fixtures.main()


if __name__ == "__main__":
    unittest.main()
