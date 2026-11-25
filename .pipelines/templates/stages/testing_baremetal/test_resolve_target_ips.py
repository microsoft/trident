# Copyright (c) Microsoft Corporation.

from pathlib import Path
import subprocess
import sys
import unittest

from resolve_target_ips import resolve_target_ips


class ResolveTargetIpsTests(unittest.TestCase):
    @staticmethod
    def target_values(index):
        return {
            "baremetal_controller.oam_ip": f"192.0.2.{index}",
            "baremetal_controller.bmc_ip": f"198.51.100.{index}",
            "baremetal_controller.oam_mac": f"02:00:00:00:00:{index:02x}",
        }

    @classmethod
    def environment(cls):
        environment = {"AGENT_NAME": "trident-bmp-lab-conn-1es-pool 12"}
        for suffix, index in (("", 100), ("_1", 1), ("_12", 12)):
            environment.update(
                {
                    name.upper().replace(".", "_") + suffix: value
                    for name, value in cls.target_values(index).items()
                }
            )
        return environment

    def test_serial_uses_unindexed_values_without_valid_agent_name(self):
        for max_parallel in (0, 1):
            for agent_name in ("", "unexpected", "trident-bmp-lab-conn-1es-pool 1"):
                with self.subTest(max_parallel=max_parallel, agent_name=agent_name):
                    environment = self.environment()
                    environment["AGENT_NAME"] = agent_name
                    self.assertEqual(
                        resolve_target_ips(environment, max_parallel),
                        self.target_values(100),
                    )

    def test_parallel_uses_indexed_values_regardless_of_unindexed_values(self):
        for explicit in (None, "", "unindexed"):
            with self.subTest(explicit=explicit):
                environment = self.environment()
                for name in self.target_values(100):
                    key = name.upper().replace(".", "_")
                    if explicit is None:
                        del environment[key]
                    else:
                        environment[key] = explicit
                self.assertEqual(
                    resolve_target_ips(environment, 2), self.target_values(12)
                )

    def test_missing_selected_values_never_fall_back(self):
        for max_parallel, suffix in ((1, ""), (2, "_12")):
            for name in self.target_values(100):
                for value in (None, ""):
                    with self.subTest(
                        max_parallel=max_parallel, name=name, value=value
                    ):
                        environment = self.environment()
                        key = name.upper().replace(".", "_") + suffix
                        if value is None:
                            del environment[key]
                        else:
                            environment[key] = value
                        with self.assertRaises(ValueError) as error:
                            resolve_target_ips(environment, max_parallel)
                        self.assertEqual(
                            str(error.exception), f"{name}{suffix} is missing or empty"
                        )

    def test_agentname_variable_is_supported(self):
        environment = self.environment()
        environment["AGENTNAME"] = "trident-bmp-lab-conn-1es-pool 1"
        self.assertEqual(resolve_target_ips(environment, 2), self.target_values(1))

    def test_parallel_requires_valid_agent_name_even_with_unindexed_values(self):
        for name in ("", "pool", "pool 0", "pool -1", "pool 1x", "1"):
            with self.subTest(name=name):
                environment = self.environment()
                environment["AGENT_NAME"] = name
                with self.assertRaisesRegex(ValueError, "1-based agent index"):
                    resolve_target_ips(environment, 2)

    def test_cli_publishes_all_resolved_values_for_later_tasks(self):
        for max_parallel, index in ((1, 100), (2, 12), (4, 12)):
            with self.subTest(max_parallel=max_parallel):
                result = self.run_script(self.environment(), max_parallel)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(
                    result.stdout.splitlines(),
                    [
                        f"##vso[task.setvariable variable={name}]{value}"
                        for name, value in self.target_values(index).items()
                    ],
                )

    def test_cli_does_not_publish_partial_results_on_failure(self):
        for max_parallel, suffix in ((1, ""), (2, "_12")):
            for name in self.target_values(100):
                for value in (None, ""):
                    with self.subTest(
                        max_parallel=max_parallel, name=name, value=value
                    ):
                        environment = self.environment()
                        key = name.upper().replace(".", "_") + suffix
                        if value is None:
                            del environment[key]
                        else:
                            environment[key] = value
                        result = self.run_script(environment, max_parallel)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertEqual(result.stdout, "")
                        self.assertIn(
                            f"{name}{suffix} is missing or empty", result.stderr
                        )

    @staticmethod
    def run_script(environment, max_parallel):
        return subprocess.run(
            [
                sys.executable,
                str(Path(__file__).with_name("resolve_target_ips.py")),
                "--max-parallel",
                str(max_parallel),
            ],
            env=environment,
            capture_output=True,
            text=True,
        )
