from ipaddress import IPv4Address
import netifaces


def get_host_default_gateway_interface() -> str:
    return netifaces.gateways()["default"][netifaces.AF_INET][1]


def get_host_default_gateway_interface_ip() -> IPv4Address:
    interface = get_host_default_gateway_interface()
    return netifaces.ifaddresses(interface)[netifaces.AF_INET][0]["addr"]
