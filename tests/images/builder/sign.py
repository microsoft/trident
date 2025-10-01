import logging
import os
import re
import subprocess
import yaml
import threading

from pathlib import Path


logging.basicConfig(level=logging.DEBUG)
log = logging.getLogger(__name__ if __name__ != "__main__" else "sign-image")

# Common name of CA (Certificate Authority) certificate
CA_CN = "Trident Testing CA"

# Name of CA certificate
CA_NAME = "trident_ca"

# Common name of signing key
KEY_CN = "Trident Testing Signing Key"

# Name of signing key
KEY_NAME = "trident_signing_key"

# Directory for the NSS key database
NSS_KEY_DB = "db"

# Name of PKCS#12 archive file that contains private key and signing certificate
PKCS12_ARCH_FILE = "signer.p12"


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
    # Export CA certificate directly from NSS database
    ca_cert_path = output_dir / "ca_cert.pem"
    subprocess.run(
        [
            "certutil",
            "-L",
            "-d",
            str(ca_nss_key_db),
            "-n",
            CA_NAME,
            "-a",
            "-o",
            str(ca_cert_path),
        ],
        check=True,
    )

    logging.info(f"CA Certificate published to {ca_cert_path}")


def sign_boot_artifacts(
    ca_nss_key_db: Path,
    leaf_key_name: str,
    inject_files_yaml_path: Path,
    output_artifacts_dir: Path,
):
    """
    Signs unsigned boot artifacts listed in inject-files.yaml and produces signed boot artifacts.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        inject_files_yaml_path: Full path to inject-files.yaml
        output_artifacts_dir: Dir where artifacts are output by Image Customizer
    """
    # Declare a list of artifacts regexes to sign; currently only UKI is signed
    artifacts_regexes = [r"vmlinuz.*\.efi"]
    # For each artifact regex, get the full path of the artifact and sign it
    for regex in artifacts_regexes:
        # Construct full path of unsigned artifact
        unsigned_artifact_path = get_artifact_path(
            inject_files_yaml_path, output_artifacts_dir, regex, False
        )

        # Construct full path of signed artifact
        signed_artifact_path = get_artifact_path(
            inject_files_yaml_path, output_artifacts_dir, regex, True
        )

        # Sign the artifact
        sign_artifact(
            ca_nss_key_db, leaf_key_name, unsigned_artifact_path, signed_artifact_path
        )


def get_artifact_path(
    inject_files_yaml_path: Path,
    output_artifacts_dir: Path,
    file_regex: str,
    signed: bool,
) -> Path:
    """
    Loads inject-files.yaml, searches each entry for a field matching the regex,
    and returns the normalized full path to the artifact.

    Args:
        inject_files_yaml_path: Path to the YAML file
        output_artifacts_dir: Directory where artifacts are stored
        file_regex: Regex to match artifact file names
        signed: If True, returns the signed artifact path, i.e. "source"; otherwise, returns the
        unsigned artifact path, i.e. "unsignedSource"

    Returns:
        Full artifact path as string if found.

    Raises:
        Exception: RuntimeError if artifact not found.
    """
    with open(inject_files_yaml_path, "r") as f:
        config = yaml.safe_load(f)

    pattern = re.compile(file_regex)

    for entry in config.get("injectFiles", []):
        if signed:
            source_type = "source"
        else:
            source_type = "unsignedSource"
        source_name = entry.get(source_type, "")
        if pattern.fullmatch(os.path.basename(source_name)):
            rel_path = source_name[2:] if source_name.startswith("./") else source_name
            return output_artifacts_dir.absolute() / rel_path

    raise RuntimeError(f"No matching entry found for pattern '{file_regex}'")


def sign_artifact(
    ca_nss_key_db: Path,
    leaf_key_name: str,
    unsigned_artifact_path: Path,
    signed_artifact_path: Path,
):
    """
    Sign the artifact using the signing key.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        unsigned_artifact_path: Path to the unsigned artifact
        signed_artifact_path: Path to the signed artifact

    Raises:
        Exception: If pesign fails.
    """
    log.debug(
        f"Process with PID {threading.get_ident()} is signing {unsigned_artifact_path} to {signed_artifact_path}"
    )

    # Create parent directory of signed artifact if it doesn't exist
    signed_artifact_path.parent.mkdir(parents=True, exist_ok=True)
    signed_artifact_path.parent.chmod(0o700)

    # Sign the artifact
    subprocess.run(
        [
            "pesign",
            "--certdir",
            str(ca_nss_key_db),
            "--certificate",
            leaf_key_name,
            "--sign",
            "--in",
            str(unsigned_artifact_path),
            "--out",
            str(signed_artifact_path),
            "--force",
        ],
        check=True,
    )
    log.debug(f"Artifact signed to {signed_artifact_path}")
