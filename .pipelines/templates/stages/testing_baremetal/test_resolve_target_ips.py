# Copyright (c) Microsoft Corporation.

from pathlib import Path
import subprocess
import sys
import unittest

from resolve_target_ips import resolve_target_ips


class ResolveTargetIpsTests(unittest.TestCase):
    def test_explicit_ips_take_precedence_without_valid_agent_name(self):
        environment = {
            "BAREMETAL_CONTROLLER_OAM_IP": "192.0.2.10",
            "BAREMETAL_CONTROLLER_BMC_IP": "192.0.2.20",
            "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
            "BAREMETAL_CONTROLLER_BMC_IP_1": "192.0.2.21",
        }
        for agent_name in ("", "unexpected", "trident-bmp-lab-conn-1es-pool 1"):
            with self.subTest(agent_name=agent_name):
                self.assertEqual(
                    resolve_target_ips(dict(environment, AGENT_NAME=agent_name)),
                    {
                        "baremetal_controller.oam_ip": "192.0.2.10",
                        "baremetal_controller.bmc_ip": "192.0.2.20",
                    },
                )

    def test_empty_or_missing_ips_use_exact_multi_digit_agent_index(self):
        for explicit in (
            {},
            {"BAREMETAL_CONTROLLER_OAM_IP": "", "BAREMETAL_CONTROLLER_BMC_IP": ""},
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
                    resolve_target_ips(environment),
                    {
                        "baremetal_controller.oam_ip": "192.0.2.12",
                        "baremetal_controller.bmc_ip": "192.0.2.22",
                    },
                )

    def test_each_ip_falls_back_independently(self):
        for direct_ip in ("OAM", "BMC"):
            with self.subTest(direct_ip=direct_ip):
                environment = {
                    "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 1",
                    "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
                    "BAREMETAL_CONTROLLER_BMC_IP_1": "192.0.2.21",
                    f"BAREMETAL_CONTROLLER_{direct_ip}_IP": "192.0.2.100",
                }
                expected = {
                    "baremetal_controller.oam_ip": "192.0.2.11",
                    "baremetal_controller.bmc_ip": "192.0.2.21",
                }
                expected[f"baremetal_controller.{direct_ip.lower()}_ip"] = "192.0.2.100"
                self.assertEqual(resolve_target_ips(environment), expected)

    def test_agentname_variable_is_supported(self):
        self.assertEqual(
            resolve_target_ips(
                {
                    "AGENTNAME": "trident-bmp-lab-conn-1es-pool 2",
                    "AGENT_NAME": "different-pool 1",
                    "BAREMETAL_CONTROLLER_OAM_IP_2": "192.0.2.12",
                    "BAREMETAL_CONTROLLER_BMC_IP_2": "192.0.2.22",
                }
            ),
            {
                "baremetal_controller.oam_ip": "192.0.2.12",
                "baremetal_controller.bmc_ip": "192.0.2.22",
            },
        )

    def test_invalid_or_missing_agent_name_fails_when_fallback_is_needed(self):
        for name in ("", "pool", "pool 0", "pool -1", "pool 1x", "1"):
            with self.subTest(name=name):
                with self.assertRaisesRegex(ValueError, "1-based agent index"):
                    resolve_target_ips({"AGENT_NAME": name})

    def test_missing_or_empty_indexed_ip_does_not_use_another_agent(self):
        for missing_ip in ("OAM", "BMC"):
            for value in (None, ""):
                with self.subTest(missing_ip=missing_ip, value=value):
                    environment = {
                        "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 2",
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
                        resolve_target_ips(environment)

    def test_cli_publishes_resolved_ips_for_later_tasks(self):
        result = self.run_script(
            {
                "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 1",
                "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
                "BAREMETAL_CONTROLLER_BMC_IP_1": "192.0.2.21",
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            [
                "##vso[task.setvariable variable=baremetal_controller.oam_ip]192.0.2.11",
                "##vso[task.setvariable variable=baremetal_controller.bmc_ip]192.0.2.21",
            ],
        )

    def test_cli_does_not_publish_partial_results_on_failure(self):
        result = self.run_script(
            {
                "AGENT_NAME": "trident-bmp-lab-conn-1es-pool 1",
                "BAREMETAL_CONTROLLER_OAM_IP_1": "192.0.2.11",
            }
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("baremetal_controller.bmc_ip_1", result.stderr)

    @staticmethod
    def run_script(environment):
        return subprocess.run(
            [sys.executable, str(Path(__file__).with_name("resolve_target_ips.py"))],
            env=environment,
            capture_output=True,
            text=True,
        )
