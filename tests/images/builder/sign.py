import logging
import os
import re
import subprocess
import yaml
import threading

from pathlib import Path
from typing import List

from builder.context_managers import temp_dir


logging.basicConfig(level=logging.DEBUG)
log = logging.getLogger(__name__)

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

# IMAGE CUSTOMIZER ARTIFACT NAMES
IC_ARTIFACT_NAME_UKIS = "ukis"
IC_ARTIFACT_NAME_SHIM = "shim"
IC_ARTIFACT_NAME_SYSTEMD_BOOT = "systemd-boot"
IC_ARTIFACT_NAME_VERITY_HASH = "verity-hash"

_KERNEL_FLAG_SUPPORTED = None
_PESIGN_CERT_ARG = None


def _efikeygen_supports_kernel_flag() -> bool:
    global _KERNEL_FLAG_SUPPORTED

    if _KERNEL_FLAG_SUPPORTED is None:
        result = subprocess.run(
            ["efikeygen", "--help"], capture_output=True, text=True, check=False
        )
        help_output = f"{result.stdout}\n{result.stderr}"
        _KERNEL_FLAG_SUPPORTED = "--kernel" in help_output
        log.debug(f"efikeygen --kernel support: {_KERNEL_FLAG_SUPPORTED}")

    return _KERNEL_FLAG_SUPPORTED


def _get_pesign_certificate_arg() -> str:
    global _PESIGN_CERT_ARG

    if _PESIGN_CERT_ARG is None:
        result = subprocess.run(
            ["pesign", "--help"], capture_output=True, text=True, check=False
        )
        help_output = f"{result.stdout}\n{result.stderr}"
        _PESIGN_CERT_ARG = "--certficate" if "--certficate" in help_output else "--certificate"
        log.debug(f"Using pesign certificate argument: {_PESIGN_CERT_ARG}")

    return _PESIGN_CERT_ARG


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

    if _efikeygen_supports_kernel_flag():
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

    # Map artifact types to file-matching regex
    item_regex = {
        IC_ARTIFACT_NAME_UKIS: r"vmlinuz.*\.efi",
        IC_ARTIFACT_NAME_SHIM: r"bootx64\.efi",
        IC_ARTIFACT_NAME_SYSTEMD_BOOT: r"systemd-bootx64\.efi",
        IC_ARTIFACT_NAME_VERITY_HASH: r".*hash.*",
    }

    # Print items to sign
    log.debug(f"Items to sign: {items_to_sign}")

    # Handle signing for each item that requires it
    for item in items_to_sign:
        regex = item_regex.get(item)
        if not regex:
            continue

        # Find unsigned and signed artifact filepaths matching this regex
        unsigned_artifact_path = get_artifact_path(
            inject_files_config, output_artifacts_dir, regex, False
        )
        signed_artifact_path = get_artifact_path(
            inject_files_config, output_artifacts_dir, regex, True
        )

        # Create parent directory of signed artifact if it doesn't exist
        signed_artifact_path.parent.mkdir(parents=True, exist_ok=True)
        signed_artifact_path.parent.chmod(0o700)

        # Specify if item is verity-hash since it requires a different signing logic
        if item == IC_ARTIFACT_NAME_VERITY_HASH:
            log.info(
                f"Signing verity hash file {unsigned_artifact_path} to {signed_artifact_path}"
            )
            sign_verity_hash(
                ca_nss_key_db,
                leaf_key_name,
                unsigned_artifact_path,
                signed_artifact_path,
            )
        else:
            log.info(
                f"Signing {item} file {unsigned_artifact_path} to {signed_artifact_path}"
            )
            sign_pe_artifact(
                ca_nss_key_db,
                leaf_key_name,
                unsigned_artifact_path,
                signed_artifact_path,
            )


def get_artifact_path(
    inject_files_config: dict,
    output_artifacts_dir: Path,
    file_regex: str,
    signed: bool,
) -> Path:
    """
    Loads inject-files.yaml, searches each entry for a field matching the regex,
    and returns the normalized full path to the artifact.

    Args:
        inject_files_config: Dictionary loaded from the YAML file
        output_artifacts_dir: Directory where artifacts are stored
        file_regex: Regex to match artifact file names
        signed: If True, returns the signed artifact path, i.e. "source"; otherwise, returns the
        unsigned artifact path, i.e. "unsignedSource"

    Returns:
        Full artifact path as string if found.

    Raises:
        Exception: RuntimeError if artifact not found.
    """
    pattern = re.compile(file_regex)

    for entry in inject_files_config.get("injectFiles", []):
        if signed:
            source_type = "source"
        else:
            source_type = "unsignedSource"
        source_name = entry.get(source_type, "")
        if pattern.fullmatch(os.path.basename(source_name)):
            rel_path = source_name[2:] if source_name.startswith("./") else source_name
            return output_artifacts_dir.absolute() / rel_path

    raise RuntimeError(f"No matching entry found for pattern '{file_regex}'")


def sign_verity_hash(
    ca_nss_key_db: Path,
    leaf_key_name: str,
    unsigned_verity_hash_path: Path,
    signed_verity_hash_path: Path,
):
    """
    Sign the verity hash file using the signing key.

    Args:
        ca_nss_key_db: Path to the NSS key database for the CA certificate
        leaf_key_name: Name of the leaf certificate
        unsigned_verity_hash_path: Path to the unsigned verity hash file
        signed_verity_hash_path: Path to the signed verity hash file

    Raises:
        Exception: If pesign fails.
    """
    log.debug(
        f"Process with PID {threading.get_ident()} is signing {unsigned_verity_hash_path}"
    )

    signed_verity_hash_path.parent.mkdir(parents=True, exist_ok=True)
    signed_verity_hash_path.parent.chmod(0o700)

    with temp_dir(sudo=True) as tmpdir:
        tmp_signed_artifact = (
            tmpdir
            / f"{unsigned_verity_hash_path.stem}.signed{unsigned_verity_hash_path.suffix}"
        )
        tmp_unsigned_artifact = (
            tmpdir
            / f"{unsigned_verity_hash_path.stem}.unsigned{unsigned_verity_hash_path.suffix}"
        )
        key_path = tmpdir / "key.p12"
        key_crt_path = tmpdir / "key.crt"

        subprocess.run(
            ["sudo", "cp", str(unsigned_verity_hash_path), str(tmp_unsigned_artifact)],
            check=True,
        )

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

        log.debug(
            f"Signing verity hash file at {tmp_unsigned_artifact} using openssl smime"
        )
        subprocess.run(
            [
                "openssl",
                "smime",
                "-sign",
                "-noattr",
                "-binary",
                "-in",
                str(tmp_unsigned_artifact),
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
            log.debug(
                f"Certs for {unsigned_verity_hash_path}:\n{result.stdout}"
            )
        except subprocess.CalledProcessError as e:
            log.error(f"Failed to print certs for {unsigned_verity_hash_path}: {e}")

        subprocess.run(
            ["sudo", "cp", str(tmp_signed_artifact), str(signed_verity_hash_path)],
            check=True,
        )

    log.debug(f"Signed verity-hash artifact generated at {signed_verity_hash_path}")


def sign_pe_artifact(
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

    with temp_dir(sudo=True) as tmpdir:
        tmp_signed_artifact = (
            tmpdir / f"{unsigned_artifact_path.stem}.signed{unsigned_artifact_path.suffix}"
        )
        tmp_unsigned_artifact = (
            tmpdir
            / f"{unsigned_artifact_path.stem}.unsigned{unsigned_artifact_path.suffix}"
        )

        subprocess.run(
            ["sudo", "cp", str(unsigned_artifact_path), str(tmp_unsigned_artifact)],
            check=True,
        )

        cert_arg = _get_pesign_certificate_arg()

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

        subprocess.run(
            ["sudo", "cp", str(tmp_signed_artifact), str(signed_artifact_path)],
            check=True,
        )

    log.debug(f"Artifact signed to {signed_artifact_path}")
