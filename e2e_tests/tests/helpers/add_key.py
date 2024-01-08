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


def add_key(trident_config_path, public_key):
    with open(trident_config_path, "r") as f:
        trident_config = yaml.safe_load(f)

    for index_user in range(
        len(trident_config["hostConfiguration"]["osconfig"]["users"])
    ):
        if (
            trident_config["hostConfiguration"]["osconfig"]["users"][index_user]["name"]
            == "testing-user"
        ):
            trident_config["hostConfiguration"]["osconfig"]["users"][index_user][
                "sshKeys"
            ].append(public_key)

    with open(trident_config_path, "w") as f:
        yaml.safe_dump(trident_config, f)


def main():
    parser = argparse.ArgumentParser(
        description="Generates RSA key and adds it to the trident configuration file."
    )
    parser.add_argument(
        "-k", "--keypath", type=str, required=True, help="Path to save the RSA key."
    )
    parser.add_argument(
        "-t",
        "--tridentconfig",
        type=str,
        required=True,
        help="Path to the trident configuration file.",
    )

    args = parser.parse_args()

    public_key = generate_rsa_key(args.keypath)
    add_key(args.tridentconfig, public_key)


if __name__ == "__main__":
    main()
