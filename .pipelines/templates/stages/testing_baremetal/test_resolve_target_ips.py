# Copyright (c) Microsoft Corporation.

from pathlib import Path
import subprocess
import sys
import unittest

from resolve_target_ips import resolve_target_ips


class ResolveTargetIpsTests(unittest.TestCase):
    def test_serial_uses_unindexed_ips_without_valid_agent_name(self):
        environment = {
            "BAREMETAL_CONTROLLER_OAM_IP": "192.0.2.10",
            "BAREMETAL_CONTROLLER_BMC_IP": "192.0.2.20",
            "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
            "BAREMETAL_CONTROLLER_BMC_IP_1": "192.0.2.21",
        }
        for max_parallel in (0, 1):
            for agent_name in ("", "unexpected", "trident-bmp-lab-conn-1es-pool 1"):
                with self.subTest(max_parallel=max_parallel, agent_name=agent_name):
                    self.assertEqual(
                        resolve_target_ips(
                            dict(environment, AGENT_NAME=agent_name), max_parallel
                        ),
                        {
                            "baremetal_controller.oam_ip": "192.0.2.10",
                            "baremetal_controller.bmc_ip": "192.0.2.20",
                        },
                    )

    def test_parallel_uses_indexed_ips_regardless_of_unindexed_values(self):
        for explicit in (
            {},
            {"BAREMETAL_CONTROLLER_OAM_IP": "", "BAREMETAL_CONTROLLER_BMC_IP": ""},
            {
                "BAREMETAL_CONTROLLER_OAM_IP": "192.0.2.100",
                "BAREMETAL_CONTROLLER_BMC_IP": "192.0.2.200",
            },
        ):
            with self.subTest(explicit=explicit):
                environment = {
                    "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 12",
                    "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
                    "BAREMETAL_CONTROLLER_BMC_IP_1": "192.0.2.21",
                    "BAREMETAL_CONTROLLER_OAM_IP_12": "192.0.2.12",
                    "BAREMETAL_CONTROLLER_BMC_IP_12": "192.0.2.22",
                    **explicit,
                }
                self.assertEqual(
                    resolve_target_ips(environment, 2),
                    {
                        "baremetal_controller.oam_ip": "192.0.2.12",
                        "baremetal_controller.bmc_ip": "192.0.2.22",
                    },
                )

    def test_serial_missing_unindexed_ips_does_not_use_indexed_values(self):
        for missing_ip in ("OAM", "BMC"):
            for value in (None, ""):
                with self.subTest(missing_ip=missing_ip, value=value):
                    environment = {
                        "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 1",
                        "BAREMETAL_CONTROLLER_OAM_IP": "192.0.2.10",
                        "BAREMETAL_CONTROLLER_BMC_IP": "192.0.2.20",
                        "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
                        "BAREMETAL_CONTROLLER_BMC_IP_1": "192.0.2.21",
                    }
                    key = f"BAREMETAL_CONTROLLER_{missing_ip}_IP"
                    if value is None:
                        del environment[key]
                    else:
                        environment[key] = value
                    with self.assertRaisesRegex(
                        ValueError,
                        f"baremetal_controller.{missing_ip.lower()}_ip is missing",
                    ):
                        resolve_target_ips(environment, 1)

    def test_agentname_variable_is_supported(self):
        self.assertEqual(
            resolve_target_ips(
                {
                    "AGENTNAME": "trident-bmp-lab-conn-1es-pool 2",
                    "AGENT_NAME": "different-pool 1",
                    "BAREMETAL_CONTROLLER_OAM_IP_2": "192.0.2.12",
                    "BAREMETAL_CONTROLLER_BMC_IP_2": "192.0.2.22",
                },
                2,
            ),
            {
                "baremetal_controller.oam_ip": "192.0.2.12",
                "baremetal_controller.bmc_ip": "192.0.2.22",
            },
        )

    def test_parallel_requires_valid_agent_name_even_with_unindexed_ips(self):
        for name in ("", "pool", "pool 0", "pool -1", "pool 1x", "1"):
            with self.subTest(name=name):
                with self.assertRaisesRegex(ValueError, "1-based agent index"):
                    resolve_target_ips(
                        {
                            "AGENT_NAME": name,
                            "BAREMETAL_CONTROLLER_OAM_IP": "192.0.2.10",
                            "BAREMETAL_CONTROLLER_BMC_IP": "192.0.2.20",
                        },
                        2,
                    )

    def test_missing_or_empty_indexed_ip_does_not_use_another_agent(self):
        for missing_ip in ("OAM", "BMC"):
            for value in (None, ""):
                with self.subTest(missing_ip=missing_ip, value=value):
                    environment = {
                        "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 2",
                        "BAREMETAL_CONTROLLER_OAM_IP": "192.0.2.10",
                        "BAREMETAL_CONTROLLER_BMC_IP": "192.0.2.20",
                        "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
                        "BAREMETAL_CONTROLLER_BMC_IP_1": "192.0.2.21",
                        "BAREMETAL_CONTROLLER_OAM_IP_2": "192.0.2.12",
                        "BAREMETAL_CONTROLLER_BMC_IP_2": "192.0.2.22",
                    }
                    key = f"BAREMETAL_CONTROLLER_{missing_ip}_IP_2"
                    if value is None:
                        del environment[key]
                    else:
                        environment[key] = value
                    with self.assertRaisesRegex(
                        ValueError, f"baremetal_controller.{missing_ip.lower()}_ip_2"
                    ):
                        resolve_target_ips(environment, 2)

    def test_cli_publishes_resolved_ips_for_later_tasks(self):
        for max_parallel, oam_ip, bmc_ip in (
            (1, "192.0.2.10", "192.0.2.20"),
            (2, "192.0.2.11", "192.0.2.21"),
            (4, "192.0.2.11", "192.0.2.21"),
        ):
            with self.subTest(max_parallel=max_parallel):
                result = self.run_script(
                    {
                        "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 1",
                        "BAREMETAL_CONTROLLER_OAM_IP": "192.0.2.10",
                        "BAREMETAL_CONTROLLER_BMC_IP": "192.0.2.20",
                        "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
                        "BAREMETAL_CONTROLLER_BMC_IP_1": "192.0.2.21",
                    },
                    max_parallel,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(
                    result.stdout.splitlines(),
                    [
                        f"##vso[task.setvariable variable=baremetal_controller.oam_ip]{oam_ip}",
                        f"##vso[task.setvariable variable=baremetal_controller.bmc_ip]{bmc_ip}",
                    ],
                )

    def test_cli_does_not_publish_partial_results_on_failure(self):
        for max_parallel, suffix, other_suffix in ((1, "", "_1"), (2, "_1", "")):
            for empty in (False, True):
                with self.subTest(max_parallel=max_parallel, empty=empty):
                    environment = {
                        "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 1",
                        "BAREMETAL_CONTROLLER_OAM_IP": "192.0.2.10",
                        "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
                        f"BAREMETAL_CONTROLLER_BMC_IP{other_suffix}": "192.0.2.20",
                    }
                    if empty:
                        environment[f"BAREMETAL_CONTROLLER_BMC_IP{suffix}"] = ""
                    result = self.run_script(environment, max_parallel)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, "")
                    self.assertIn(
                        f"baremetal_controller.bmc_ip{suffix} is missing or empty",
                        result.stderr,
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
