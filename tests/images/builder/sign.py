import logging
import os
import re
import shutil
import subprocess
import yaml
import threading

from contextlib import ExitStack
from pathlib import Path
from typing import List, Optional

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

    # Compose command only adding --kernel flag if it is required by the
    # current efikeygen version in the image
    result = subprocess.run(
        ["efikeygen", "--help"], capture_output=True, text=True, check=False
    )
    supports_kernel = "--kernel" in result.stdout

    # Compose command, conditionally adding --kernel flag
    cmd = [
        "efikeygen",
        "-n",
        leaf_key_name,
        "-c",
        f"CN={KEY_CN} {id}",
        "--signer",
        CA_NAME,
        "-d",
        str(ca_nss_key_db),
    ]

    if supports_kernel:
        cmd.append("--kernel")

    # Generate signing key/cert, signed by CA in the shared DB
    subprocess.run(cmd, check=True)

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
    inject_files_yaml_path: Path,
    output_artifacts_dir: Path,
    stack: ExitStack,
):
    """
    Signs unsigned boot artifacts listed in inject-files.yaml and produces signed boot artifacts.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        inject_files_yaml_path: Full path to inject-files.yaml
        output_artifacts_dir: Dir where artifacts are output by Image Customizer
        stack: ExitStack, used to manage temporary directories
    """
    # Print contents of inject_files_yaml_path
    with open(inject_files_yaml_path, "r") as f:
        data = f.read()

    log.debug(f"Contents of {inject_files_yaml_path}:\n{data}")
    inject_files_config = yaml.safe_load(data)

    # Handle signing for each item that requires it
    for entry in inject_files_config.get("injectFiles", []):
        artifact_type = entry.get("type", "")
        signed_path_str = entry.get("source")
        unsigned_path_str = entry.get("unsignedSource", "")

        if unsigned_path_str == "":
            # MIC v1.1+ use the same path for unsigned and signed files.
            unsigned_path_str = signed_path_str

        signed_path = output_artifacts_dir.absolute() / signed_path_str
        unsigned_path = output_artifacts_dir.absolute() / unsigned_path_str

        if artifact_type == "":
            # MIC v1.1+ provides the artifact type field.
            # For other versions, derive it from the file name.
            artifact_type = get_artifact_type_from_name(unsigned_path.name)

        # Check if item is verity-hash since it requires a different signing logic
        log.info(f"Signing file of type '{artifact_type}' at {unsigned_path}")
        if artifact_type == IC_ARTIFACT_NAME_VERITY_HASH:
            sign_verity_hash(
                ca_nss_key_db, leaf_key_name, unsigned_path, signed_path, stack
            )
        else:
            sign_pe_artifact(
                ca_nss_key_db, leaf_key_name, unsigned_path, signed_path, stack
            )


def get_artifact_type_from_name(name: str) -> Optional[str]:
    if re.match(r"vmlinuz.*\.efi", name):
        return IC_ARTIFACT_NAME_UKIS
    elif re.match(r"bootx64\.efi", name):
        return IC_ARTIFACT_NAME_SHIM
    elif re.match(r"systemd-bootx64\.efi", name):
        return IC_ARTIFACT_NAME_SYSTEMD_BOOT
    elif re.match(r".*hash.*", name):
        return IC_ARTIFACT_NAME_VERITY_HASH

    return None


def sign_verity_hash(
    ca_nss_key_db: Path,
    leaf_key_name: str,
    unsigned_path: Path,
    signed_path: Path,
    stack: ExitStack,
):
    """
    Sign the verity hash file using the signing key.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        unsigned_path: Path to the unsigned verity hash file
        signed_path: Path to the signed verity hash file
        stack: ExitStack, used to manage temporary directories

    Raises:
        Exception: If pesign fails.
    """
    log.debug(f"Process with PID {threading.get_ident()} is signing {unsigned_path}")

    # Create a temp dir to store temp signed artifact
    tmp_dir = stack.enter_context(temp_dir(sudo=True))
    # Create a file inside the temp dir to store the signed artifact
    tmp_signed_artifact = tmp_dir / f"{unsigned_path.stem}.signed{unsigned_path.suffix}"
    # Create a file inside the temp dir to store the unsigned artifact
    tmp_unsigned_artifact = (
        tmp_dir / f"{unsigned_path.stem}.unsigned{unsigned_path.suffix}"
    )
    shutil.copy(str(unsigned_path), str(tmp_unsigned_artifact))

    # Sign the verity hash file
    key_path = tmp_dir / "key.p12"
    key_crt_path = tmp_dir / "key.crt"

    # Export PKCS12 key
    log.debug(f"Exporting PKCS12 key to {key_path}")
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
    log.debug(f"Extracting cert from PKCS12 key to {key_crt_path}")
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

    # openssl smime sign
    log.debug(f"Signing verity hash file at {unsigned_path} using openssl smime")
    subprocess.run(
        [
            "openssl",
            "smime",
            "-sign",
            "-noattr",
            "-binary",
            "-in",
            str(unsigned_path),
            "-signer",
            str(key_crt_path),
            "-passin",
            "pass:",
            "-outform",
            "der",
            "-out",
            str(tmp_signed_artifact),
        ],
        check=True,
    )

    # Print certs for debug/validation
    try:
        result = subprocess.run(
            [
                "openssl",
                "pkcs7",
                "-inform",
                "DER",
                "-in",
                str(tmp_signed_artifact),
                "-print_certs",
                "-text",
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        log.debug(f"Certs for {unsigned_path}:\n{result.stdout}")

    except subprocess.CalledProcessError as e:
        log.error(f"Failed to print certs for {unsigned_path}: {e}")

    # Finally, write the signed artifact to the original path
    shutil.copy(str(tmp_signed_artifact), str(signed_path))
    log.debug(f"Signed verity-hash artifact generated at {signed_path}")


def sign_pe_artifact(
    ca_nss_key_db: Path,
    leaf_key_name: str,
    unsigned_path: Path,
    signed_path: Path,
    stack: ExitStack,
):
    """
    Sign the artifact using the signing key.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        unsigned_path: Path to the unsigned verity hash file
        signed_path: Path to the signed verity hash file
        stack: ExitStack, used to manage temporary directories

    Raises:
        Exception: If pesign fails.
    """
    log.debug(f"Process with PID {threading.get_ident()} is signing {unsigned_path}")

    # Create a temp dir to store temp signed artifact
    tmp_dir = stack.enter_context(temp_dir(sudo=True))

    # Create a file inside the temp dir to store the signed artifact
    tmp_signed_artifact = tmp_dir / f"{unsigned_path.stem}.signed{unsigned_path.suffix}"
    # Create a file inside the temp dir to store the unsigned artifact
    tmp_unsigned_artifact = (
        tmp_dir / f"{unsigned_path.stem}.unsigned{unsigned_path.suffix}"
    )

    # Copy the unsigned file to temp directory using sudo since source is root-owned
    subprocess.run(
        ["sudo", "cp", str(unsigned_path), str(tmp_unsigned_artifact)], check=True
    )

    log.debug(f"Signing PE artifact at {unsigned_path} using pesign")

    # In older pesign versions, e.g. in Ubuntu 22, --certificate arg is
    # misspelled as --certficate, so check which one to use.
    result = subprocess.run(
        ["pesign", "--help"], capture_output=True, text=True, check=False
    )
    cert_arg = "--certificate"
    if "--certficate" in result.stdout:
        cert_arg = "--certficate"

    # Sign as a PE binary
    subprocess.run(
        [
            "pesign",
            "--certdir",
            str(ca_nss_key_db),
            cert_arg,
            leaf_key_name,
            "--sign",
            "--in",
            str(tmp_unsigned_artifact),
            "--out",
            str(tmp_signed_artifact),
            "--force",
        ],
        check=True,
    )

    # Copy the signed file back using sudo since destination is root-owned
    subprocess.run(
        ["sudo", "cp", str(tmp_signed_artifact), str(signed_path)], check=True
    )
    log.debug(f"Signed PE artifact generated at {signed_path}")
