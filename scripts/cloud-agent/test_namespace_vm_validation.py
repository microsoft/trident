import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import MagicMock, patch

SPEC = importlib.util.spec_from_file_location(
    "validation", Path(__file__).with_name("namespace-vm-validation.py")
)
validation = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validation)


def events(*, outcome="pass", marker=True):
    package, name = validation.VM_PACKAGE, validation.VM_CASE
    values = [
        {"Package": package, "Test": name, "Action": "run"},
        {
            "Package": package,
            "Test": name,
            "Action": "output",
            "Output": validation.VM_PROOF if marker else "child failed intentionally",
        },
        {"Package": package, "Test": name, "Action": outcome},
        {"Package": package, "Action": "pass"},
    ]
    return "\n".join(map(json.dumps, values))


class CaseResultsTests(unittest.TestCase):
    def parse(self, text):
        return validation.case_results(
            text, validation.VM_PACKAGE, {validation.VM_CASE}, vm=True
        )

    def test_outer_pass_counts_once(self):
        self.assertEqual(self.parse(events())["passed"], 1)

    def test_skip_and_failure_rejected(self):
        for outcome in ("skip", "fail"):
            with self.subTest(outcome=outcome), self.assertRaises(ValueError):
                self.parse(events(outcome=outcome))

    def test_child_failure_text_is_not_execution_proof(self):
        with self.assertRaises(ValueError):
            self.parse(events(marker=False))

    def test_collection_only_rejected(self):
        with self.assertRaises(ValueError):
            self.parse(json.dumps({"Package": validation.VM_PACKAGE, "Action": "pass"}))

    def test_missing_run_duplicate_and_wrong_package_rejected(self):
        text = events()
        for invalid in (
            "\n".join(text.splitlines()[1:]),
            text + "\n" + text,
            text.replace(validation.VM_PACKAGE, "wrong"),
            "not JSON",
        ):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                self.parse(invalid)

    def test_all_regression_subcases_counted(self):
        names = sorted(validation.REGRESSIONS) + [
            "TestCleanupNamespacePropagatesErrors/list_domains"
        ]
        values = []
        for name in names:
            for action in ("run", "pass"):
                values.append(
                    {
                        "Package": "tridenttools/pkg/virtdeploy",
                        "Test": name,
                        "Action": action,
                    }
                )
        values.append({"Package": "tridenttools/pkg/virtdeploy", "Action": "pass"})
        result = validation.case_results(
            "\n".join(map(json.dumps, values)),
            "tridenttools/pkg/virtdeploy",
            validation.REGRESSIONS,
        )
        self.assertEqual(result["passed"], 7)
        self.assertEqual(result["skipped"], 0)

    def test_inventory_uses_supported_commands(self):
        instance = object.__new__(validation.Validation)
        with patch.object(instance, "run", return_value=""), patch.object(
            instance, "save"
        ):
            self.assertEqual(instance.inventory("before")["domains"], [])
            commands = [call.args[1] for call in instance.run.call_args_list]
        self.assertEqual(
            [cmd[5] for cmd in commands[:3]], ["list", "pool-list", "net-list"]
        )
        self.assertNotIn("vol-list", str(commands))

    def test_subprocess_failure_propagates_even_with_pass_text(self):
        instance = object.__new__(validation.Validation)
        instance.source, instance.env, instance.reports = ".", {}, MagicMock()
        (instance.reports / "failure-vm.jsonl").read_text.return_value = events()
        with patch.object(instance, "save"), patch.object(
            validation.subprocess, "run", return_value=MagicMock(returncode=1)
        ):
            with self.assertRaises(RuntimeError):
                instance.run("failure-vm", ["false"])

    def test_dependency_stderr_is_separate_from_json(self):
        instance = object.__new__(validation.Validation)
        instance.source, instance.env, instance.reports = ".", {}, MagicMock()
        files = {
            "virtdeploy.jsonl": MagicMock(),
            "virtdeploy.stderr.log": MagicMock(),
        }
        instance.reports.__truediv__.side_effect = files.__getitem__
        stdout = files["virtdeploy.jsonl"]
        stdout.read_text.return_value = events()
        with patch.object(instance, "save"), patch.object(
            validation.subprocess, "run", return_value=MagicMock(returncode=0)
        ) as run:
            self.assertEqual(instance.run("virtdeploy", ["go", "test"]), events())
            self.assertIsNot(
                run.call_args.kwargs["stdout"], run.call_args.kwargs["stderr"]
            )

    def test_audit_fails_on_changed_inventory_or_unverifiable_teardown(self):
        for inventory in ({"domains": ["leaked-vm"]}, RuntimeError("virsh failed")):
            instance = object.__new__(validation.Validation)
            instance.reports = MagicMock()
            (instance.reports / "inventory-before.json").read_text.return_value = "{}"
            with patch.object(
                instance,
                "inventory",
                **(
                    {"side_effect": inventory}
                    if isinstance(inventory, Exception)
                    else {"return_value": inventory}
                )
            ), patch.object(instance, "clean_source"), patch.object(
                instance, "run"
            ), patch.object(
                instance, "save"
            ) as save:
                with self.assertRaises(RuntimeError):
                    instance.audit()
                self.assertFalse(save.call_args.args[1]["passed"])


if __name__ == "__main__":
    unittest.main()
