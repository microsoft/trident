import importlib.util
import ipaddress
import unittest

from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "select_functional_network.py"
SPEC = importlib.util.spec_from_file_location("select_functional_network", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class FunctionalNetworkTests(unittest.TestCase):
    def test_libvirt_xml_networks_supports_prefix_and_netmask(self):
        xml = """
        <network>
          <ip address="192.168.122.1" netmask="255.255.255.0"/>
          <ip family="ipv4" address="172.24.0.1" prefix="16"/>
          <ip family="ipv6" address="fd00::1" prefix="64"/>
        </network>
        """

        self.assertEqual(
            MODULE.libvirt_xml_networks(xml),
            [
                ipaddress.ip_network("192.168.122.0/24"),
                ipaddress.ip_network("172.24.0.0/16"),
            ],
        )

    def test_select_network_is_deterministic_and_skips_overlaps(self):
        namespace = "trident-functional-test"
        first_candidate = next(MODULE.candidate_networks(namespace))
        unavailable_networks = [
            ipaddress.ip_network("10.0.0.0/8"),
            first_candidate,
        ]

        selected = MODULE.select_network(namespace, unavailable_networks)

        self.assertEqual(
            selected,
            MODULE.select_network(namespace, unavailable_networks),
        )
        self.assertFalse(
            any(selected.overlaps(network) for network in unavailable_networks)
        )
        self.assertEqual(selected.prefixlen, 24)

    def test_select_network_rejects_overlapping_override(self):
        with self.assertRaisesRegex(
            MODULE.NetworkSelectionError, "overlaps an existing"
        ):
            MODULE.select_network(
                "namespace",
                [ipaddress.ip_network("192.168.50.0/24")],
                "192.168.50.0/24",
            )

    def test_select_network_accepts_available_override(self):
        self.assertEqual(
            MODULE.select_network(
                "namespace",
                [ipaddress.ip_network("10.0.0.0/8")],
                "192.168.50.0/24",
            ),
            ipaddress.ip_network("192.168.50.0/24"),
        )
