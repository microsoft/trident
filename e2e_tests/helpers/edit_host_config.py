import argparse
from cryptography.hazmat.primitives import serialization as crypto_serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.hazmat.backends import default_backend as crypto_default_backend
import yaml


def generate_rsa_key(path):
    key = rsa.generate_private_key(
        backend=crypto_default_backend(), public_exponent=65537, key_size=2048
    )
    private_key = key.private_bytes(
        crypto_serialization.Encoding.PEM,
        crypto_serialization.PrivateFormat.PKCS8,
        crypto_serialization.NoEncryption(),
    )
    public_key = key.public_key().public_bytes(
        crypto_serialization.Encoding.OpenSSH, crypto_serialization.PublicFormat.OpenSSH
    )

    with open(path, "wb") as f:
        f.write(private_key)

    return public_key.decode("utf-8")


def add_key(host_config_path, public_key):
    with open(host_config_path, "r") as f:
        host_config = yaml.safe_load(f)

    for index_user in range(len(host_config["os"]["users"])):
        if host_config["os"]["users"][index_user]["name"] == "testing-user":
            host_config["os"]["users"][index_user]["sshPublicKeys"].append(public_key)

    with open(host_config_path, "w") as f:
        yaml.safe_dump(host_config, f)


def add_copy_command(host_config_path):
    with open(host_config_path, "r") as f:
        host_config = yaml.safe_load(f)

    if "os" not in host_config:
        host_config["os"] = {}
    if "additionalFiles" not in host_config["os"]:
        host_config["os"]["additionalFiles"] = []

    host_config["os"]["additionalFiles"].append({})
    host_config["os"]["additionalFiles"][-1][
        "source"
    ] = "/var/lib/trident/trident-container.tar.gz"
    host_config["os"]["additionalFiles"][-1][
        "destination"
    ] = "/var/lib/trident/trident-container.tar.gz"

    with open(host_config_path, "w") as f:
        yaml.safe_dump(host_config, f)


# Images stored in ACR are tagged based on pipeline build ID, and therefore the
# URL must be updated for every build.
def rename_oci_url(host_config_path, oci_url):
    with open(host_config_path, "r") as f:
        host_config = yaml.safe_load(f)

    host_config["image"]["url"] = oci_url

    with open(host_config_path, "w") as f:
        yaml.safe_dump(host_config, f)


def main():
    parser = argparse.ArgumentParser(
        description="Makes host configuration edits: Adds an SSH key and optionally copies the container image."
    )
    parser.add_argument(
        "-k", "--keypath", type=str, required=True, help="Path to save the RSA key."
    )
    parser.add_argument(
        "-t",
        "--hostconfig",
        type=str,
        required=True,
        help="Path to the Trident configuration file.",
    )
    parser.add_argument(
        "-o",
        "--ociUrl",
        type=str,
        required=False,
        help="Url to ACR blob containing COSI file.",
    )
    parser.add_argument(
        "-r",
        "--runtimeEnv",
        type=str,
        required=True,
        choices=["host", "container"],
        help="The runtime environment of Trident (e.g., host or container).",
    )
    args = parser.parse_args()

    public_key = generate_rsa_key(args.keypath)
    add_key(args.hostconfig, public_key)

    if args.runtimeEnv == "container":
        add_copy_command(args.hostconfig)

    if args.ociUrl:
        rename_oci_url(args.hostconfig, args.ociUrl)


if __name__ == "__main__":
    main()
