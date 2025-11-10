import logging
import os
import shutil
import subprocess
import yaml
import threading

from pathlib import Path
from typing import List

from builder.context_managers import temp_dir


logging.basicConfig(level=logging.DEBUG)
log = logging.getLogger(__name__)

# Common name of CA (Certificate Authority) certificate
CA_CN = "Internal Test Ephemeral CA"

# Name of CA certificate
CA_NAME = "ephemeral_ca"

# Common name of signing key
KEY_CN = "Internal Test Ephemeral Signing Key"

# Name of signing key
KEY_NAME = "ephemeral_signing_key"

# Directory for the NSS key database
NSS_KEY_DB = "db"

# Name of PKCS#12 archive file that contains private key and signing certificate
PKCS12_ARCH_FILE = "signer.p12"

# IMAGE CUSTOMIZER ARTIFACT NAMES
IC_ARTIFACT_NAME_UKIS = "ukis"
IC_ARTIFACT_NAME_SHIM = "shim"
IC_ARTIFACT_NAME_SYSTEMD_BOOT = "systemd-boot"
IC_ARTIFACT_NAME_VERITY_HASH = "verity-hash"


def generate_ca_certificate(tmp_dir: Path):
    """
    Generates a single CA certificate and key that will be used to sign all leaf certificates.
    This should be called once per build process.

    Args:
        tmp_dir: Path to the temporary directory where the CA key is stored

    Returns:
        ca_nss_key_db: Full path to the NSS key database

    Raises:
        Exception: If certutil or efikeygen fails.
    """
    ca_nss_key_db = tmp_dir / NSS_KEY_DB
    os.makedirs(ca_nss_key_db, exist_ok=True)
    log.debug(f"Initializing CA NSS key database in {ca_nss_key_db}")

    # Initialize a NSS key database for CA
    subprocess.run(
        ["certutil", "-N", "-d", str(ca_nss_key_db), "--empty-password"], check=True
    )

    # Generate CA certificate
    subprocess.run(
        [
            "efikeygen",
            "-C",
            "-S",
            "-n",
            CA_NAME,
            "-c",
            f"CN={CA_CN}",
            "-d",
            str(ca_nss_key_db),
        ],
        check=True,
    )

    log.info(f"Generated CA certificate {CA_NAME} at {ca_nss_key_db}")
    return ca_nss_key_db


def generate_leaf_certificate(ca_nss_key_db: Path, id: str):
    """
    Generates a leaf certificate signed by the CA for a specific image clone.

    Args:
        ca_nss_key_db: Path to the CA's NSS key database
        id: ID of the signing key for this image clone

    Returns:
        leaf_key_name: Name of the leaf key that was generated

    Raises:
        Exception: If certificate generation fails.
    """
    # Generate unique leaf key name
    leaf_key_name = f"{KEY_NAME}_{id}"

    # Generate signing key/cert, signed by CA in the shared DB
    subprocess.run(
        [
            "efikeygen",
            "-n",
            leaf_key_name,
            "-c",
            f"CN={KEY_CN} {id}",
            "--signer",
            CA_NAME,
            "-d",
            str(ca_nss_key_db),
            "--kernel",
        ],
        check=True,
    )

    log.debug(
        f"Process with PID {threading.get_ident()} generated leaf key {leaf_key_name} in {ca_nss_key_db}"
    )
    return leaf_key_name


def publish_ca_certificate(ca_nss_key_db: Path, output_dir: Path):
    """
    Extract and publish the CA certificate that can validate all leaf certificates.

    Args:
        ca_nss_key_db: Path to the CA's NSS key database
        output_dir: Directory where the CA certificate will be published

    Raises:
        Exception: If any shell command fails.
    """
    # Export PKCS#12 from NSS DB
    key_path = ca_nss_key_db / PKCS12_ARCH_FILE
    subprocess.run(
        [
            "pk12util",
            "-d",
            str(ca_nss_key_db),
            "-n",
            CA_NAME,
            "-o",
            str(key_path),
            "-W",
            "",
        ],
        check=True,
    )

    # Extract certificate from PKCS#12 file, no private key
    ca_cert_path = output_dir / "ca_cert.pem"
    subprocess.run(
        [
            "openssl",
            "pkcs12",
            "-in",
            str(key_path),
            "-out",
            str(ca_cert_path),
            "-nokeys",
            "-passin",
            "pass:",
        ],
        check=True,
    )

    log.info(f"CA Certificate published to {ca_cert_path}")


def sign_boot_artifacts(
    ca_nss_key_db: Path,
    leaf_key_name: str,
    items_to_sign: List[str],
    inject_files_yaml_path: Path,
    output_artifacts_dir: Path,
):
    """
    Signs unsigned boot artifacts listed in inject-files.yaml and produces signed boot artifacts.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        items_to_sign: List of items to sign
        inject_files_yaml_path: Full path to inject-files.yaml
        output_artifacts_dir: Dir where artifacts are output by Image Customizer
    """
    # Print contents of inject_files_yaml_path
    with open(inject_files_yaml_path, "r") as f:
        data = f.read()

    log.debug(f"Contents of {inject_files_yaml_path}:\n{data}")
    inject_files_config = yaml.safe_load(data)

    # Print items to sign
    log.debug(f"Items to sign: {items_to_sign}")

    # Handle signing for each item that requires it
    for item in items_to_sign:
        try:
            # Find artifact filepath for this item type
            artifact_path = get_artifact_path(
                inject_files_config, output_artifacts_dir, item
            )

            # Create parent directory if it doesn't exist
            artifact_path.parent.mkdir(parents=True, exist_ok=True)
            artifact_path.parent.chmod(0o700)

            # Specify if item is verity-hash since it requires a different signing logic
            log.info(f"Signing {item} file at {artifact_path}")
            if item == IC_ARTIFACT_NAME_VERITY_HASH:
                sign_verity_hash(
                    ca_nss_key_db,
                    leaf_key_name,
                    artifact_path,
                )
            else:
                sign_pe_artifact(
                    ca_nss_key_db,
                    leaf_key_name,
                    artifact_path,
                )
        except Exception as e:
            log.warning(f"Failed to sign artifact item '{item}': {e}")
            continue


def get_artifact_path(
    inject_files_config: dict,
    output_artifacts_dir: Path,
    item_type: str,
) -> Path:
    """
    Loads inject-files.yaml, searches each entry for a matching file type, and
    returns the normalized full path to the artifact.

    Args:
        inject_files_config: Dictionary loaded from the YAML file
        output_artifacts_dir: Directory where artifacts are stored
        item_type: Type of artifact to search for, i.e. "ukis", "shim",
        "systemd-boot", or "verity-hash"

    Returns:
        Full artifact path as string if found.

    Raises:
        Exception: RuntimeError if artifact not found.
    """
    for entry in inject_files_config.get("injectFiles", []):
        if entry.get("type") == item_type:
            source_name = entry.get("source", "")
            if source_name:
                rel_path = (
                    source_name[2:] if source_name.startswith("./") else source_name
                )
                return output_artifacts_dir.absolute() / rel_path

    raise RuntimeError(f"No matching entry found for type '{item_type}'")


def sign_verity_hash(
    ca_nss_key_db: Path,
    leaf_key_name: str,
    artifact_path: Path,
):
    """
    Sign the verity hash file using the signing key.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        artifact_path: Path to the unsigned/signed verity hash file

    Raises:
        Exception: If pesign fails.
    """
    with temp_dir() as tmpdir:
        # Sign the verity hash file
        key_path = tmpdir / "key.p12"
        key_crt_path = tmpdir / "key.crt"
        # Export PKCS12 key
        subprocess.run(
            [
                "pk12util",
                "-d",
                str(ca_nss_key_db),
                "-n",
                leaf_key_name,
                "-o",
                str(key_path),
                "-W",
                "",
            ],
            check=True,
        )
        # Extract cert
        subprocess.run(
            [
                "openssl",
                "pkcs12",
                "-in",
                str(key_path),
                "-out",
                str(key_crt_path),
                "-clcerts",
                "-nodes",
                "-passin",
                "pass:",
            ],
            check=True,
        )

        # Create a temporary file to store the signed verity hash
        temp_artifact_path = artifact_path.parent / (artifact_path.name + ".signed")

        # smime sign
        subprocess.run(
            [
                "openssl",
                "smime",
                "-sign",
                "-noattr",
                "-binary",
                "-in",
                str(artifact_path),
                "-signer",
                str(key_crt_path),
                "-passin",
                "pass:",
                "-outform",
                "der",
                "-out",
                str(temp_artifact_path),
            ],
            check=True,
        )

        # Print certs for debug/validation as in bash
        subprocess.run(
            [
                "openssl",
                "pkcs7",
                "-inform",
                "DER",
                "-in",
                str(temp_artifact_path),
                "-print_certs",
                "-text",
            ],
            check=True,
        )
        # Finally, write the signed verity hash to the original path
        shutil.move(str(temp_artifact_path), str(artifact_path))
        log.debug(f"Artifact signed to {artifact_path}")


def sign_pe_artifact(
    ca_nss_key_db: Path,
    leaf_key_name: str,
    artifact_path: Path,
):
    """
    Sign the artifact using the signing key.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        artifact_path: Path to the unsigned/signed artifact

    Raises:
        Exception: If pesign fails.
    """
    log.debug(f"Process with PID {threading.get_ident()} is signing {artifact_path}")

    # Create a temporary file to store the signed artifact
    temp_artifact_path = artifact_path.parent / (artifact_path.name + ".signed")

    # Sign as a PE binary
    subprocess.run(
        [
            "pesign",
            "--certdir",
            str(ca_nss_key_db),
            "--certificate",
            leaf_key_name,
            "--sign",
            "--in",
            str(artifact_path),
            "--out",
            str(temp_artifact_path),
            "--force",
        ],
        check=True,
    )

    # Finally, write the signed artifact to the original path
    shutil.move(str(temp_artifact_path), str(artifact_path))
    log.debug(f"Artifact signed to {artifact_path}")
