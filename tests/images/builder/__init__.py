import logging
import yaml

from dataclasses import dataclass, field, fields
from enum import Enum
from pathlib import Path
from typing import List, Optional, Union

log = logging.getLogger(__name__)


class Distro(Enum):
    AZL3 = "azl3"
    AZL4 = "azl4"
    OTHER = "other"


@dataclass
class BaseImageData:
    name: str
    path: Path
    mcr_name: Optional[str] = None
    distro: Distro = Distro.AZL3


class BaseImage(Enum):
    BAREMETAL = BaseImageData("baremetal", Path("artifacts/baremetal.vhdx"))
    CORE_SELINUX = BaseImageData("core_selinux", Path("artifacts/core_selinux.vhdx"))
    QEMU_GUEST = BaseImageData("qemu_guest", Path("artifacts/qemu_guest.vhdx"))
    AZL4_QEMU_GUEST = BaseImageData(
        "azl4_qemu_guest",
        Path("artifacts/azl4_qemu_guest.vhdx"),
        distro=Distro.AZL4,
    )
    # AZL4_CORE = BaseImageData(
    #     "azl4_core", Path("artifacts/azl4_core.vhdx"), "core", Distro.AZL4
    # )
    CORE_ARM64 = BaseImageData("core_arm64", Path("artifacts/core_arm64.vhdx"))
    MINIMAL = BaseImageData("minimal", Path("artifacts/minimal.vhdx"))
    MINIMAL_AARCH64 = BaseImageData(
        "minimal_aarch64", Path("artifacts/minimal_aarch64.vhdx")
    )
    UBUNTU_2204_AMD64 = BaseImageData(
        "ubuntu_2204_amd64",
        Path("artifacts/ubuntu_2204_amd64.vhdx"),
        distro=Distro.OTHER,
    )
    UBUNTU_2204_ARM64 = BaseImageData(
        "ubuntu_2204_arm64",
        Path("artifacts/ubuntu_2204_arm64.vhdx"),
        distro=Distro.OTHER,
    )
    UBUNTU_2404_AMD64 = BaseImageData(
        "ubuntu_2404_amd64",
        Path("artifacts/ubuntu_2404_amd64.vhdx"),
        distro=Distro.OTHER,
    )
    UBUNTU_2404_ARM64 = BaseImageData(
        "ubuntu_2404_arm64",
        Path("artifacts/ubuntu_2404_arm64.vhdx"),
        distro=Distro.OTHER,
    )
    GB200_2404_ARM64 = BaseImageData(
        "gb200_2404_arm64", Path("artifacts/gb200_2404_arm64.vhdx"), distro=Distro.OTHER
    )

    @property
    def path(self) -> Path:
        return self.value.path

    @property
    def name(self) -> str:
        return self.value.name

    @property
    def mcr_name(self) -> str:
        if self.value.mcr_name is not None:
            return self.value.mcr_name
        return self.value.name

    def __str__(self) -> str:
        return self.value.name


@dataclass
class BaseImageManifest:
    image: BaseImage
    package_name: str
    version: str
    distro: Distro = Distro.AZL3
    org: str = "https://dev.azure.com/mariner-org/"
    project: str = "36d030d6-1d99-4ebd-878b-09af1f4f722f"
    feed: str = "AzureLinuxArtifacts"
    glob: str = "*.vhdx"


@dataclass
class BlobImageManifest:
    """Manifest for a base image fetched from Azure Storage Blob.

    Used for distros that don't yet publish to an ADO universal artifact
    feed (e.g., Azure Linux 4.0 alpha builds). The storage account name
    and container are NOT baked in here -- they are supplied at
    invocation time via the --blob-storage-account / --blob-container
    flags (or the BLOB_STORAGE_ACCOUNT / BLOB_CONTAINER env vars) so the
    pipeline can parameterize them and rotate the location without a
    code change.

    Authentication is via `az` CLI logged-in identity (`--auth-mode
    login`). The pipeline running this must have a federated identity
    with read access to the storage account.
    """

    image: BaseImage
    # Blob name prefix to search under
    # (e.g. "azure-linux/core-efi-vhdx-4.0-amd64")
    path_prefix: str
    # Suffix the final blob name must end with.
    # The downloader lists all blobs under path_prefix, filters to ones
    # ending with this suffix, and picks the lexically largest (= most
    # recent version) to download.
    file_suffix: str = "/image.vhdx"


class OutputFormat(Enum):
    BAREMETAL_IMAGE = "baremetal-image"
    COSI = "cosi"
    VHDX = "vhdx"
    RAW = "raw"
    QCOW2 = "qcow2"
    ISO = "iso"
    VHD = "vhd"
    VHD_FIXED = "vhd-fixed"

    def ic_name(self):
        """Return the Image Customizer name for this format."""
        return self.value

    def ext(self) -> str:
        """Return the file extension for this format."""
        if self == OutputFormat.VHD_FIXED:
            return "vhd"
        elif self == OutputFormat.BAREMETAL_IMAGE:
            return "cosi"
        return self.value


class RpmSources(Enum):
    TRIDENT = Path("bin/RPMS")
    DHCP = Path("artifacts/dhcp")
    RPM_OVERRIDES = Path("artifacts/rpm-overrides")

    def path(self) -> Path:
        return self.value


class SystemArchitecture(Enum):
    AMD64 = "amd64"
    ARM64 = "arm64"

    def __str__(self) -> str:
        return self.value


@dataclass
class ImageConfig:
    # Friendly name of the image
    name: str

    # Top level config dir
    source: str = "tests/images"

    # Second-level dir, generally same as name
    config: str = None

    # The base image to use
    base_image: BaseImage = BaseImage.BAREMETAL

    # Whether the image requires Trident RPMs
    requires_trident: bool = True

    # Whether the image requires DHCP RPMs
    requires_dhcp: bool = False

    # Desired output format for this image
    output_and_config: dict[OutputFormat, Path] = field(
        default_factory=lambda: {OutputFormat.COSI: Path("base/baseimg.yaml")}
    )

    # Extra dependencies for this image
    extra_dependencies: List[Path] = field(default_factory=list)

    # Requires ukify to be present on the host
    requires_ukify: bool = False

    # When present, path to write a public SSH key to for customizer to consume
    # into the image. Both keys will be written to the output directory.
    ssh_key: Optional[Path] = None

    # Architecture of the image
    architecture: SystemArchitecture = SystemArchitecture.AMD64

    # Use ImageCustomizer convert command rather than customize
    image_customizer_convert: bool = False

    # Runtime variable used to configure output format
    runtime_output_format: Optional[OutputFormat] = None

    @classmethod
    def kebab_fields(cls) -> List[str]:
        """Return a list of fields in kebab-case."""
        return [f.name.replace("_", "-") for f in fields(cls)]

    def __post_init__(self):
        self.suffix = None
        if not self.config:
            self.config = self.name

        # Update the ssh key to be a Path object if it's a string
        if isinstance(self.ssh_key, str):
            self.ssh_key = Path(self.ssh_key)

        # Normalize output_and_config values to Path objects
        for fmt in self.output_and_config:
            cfg = self.output_and_config[fmt]
            if isinstance(cfg, str):
                self.output_and_config[fmt] = Path(cfg)

        # Automatically set the architecture to arm64 if the base image is ARM64
        if self.base_image == BaseImage.CORE_ARM64:
            self.architecture = SystemArchitecture.ARM64

        # Placeholder for the loaded base Image Customizer config
        self.__base_ic_config = None

    @property
    def base_ic_config(self) -> dict:
        """Lazy-load and return the base Image Customizer config as a dict."""
        if self.__base_ic_config is None:
            try:
                with open(self.full_yaml_path(), "r") as f:
                    self.__base_ic_config = yaml.safe_load(f)
            except Exception as e:
                raise RuntimeError(
                    f"Error loading image config '{self.full_yaml_path()}': {e}"
                ) from e
        return self.__base_ic_config

    def base_dir(self) -> Path:
        return Path(self.source) / self.config

    def output_format(self) -> OutputFormat:
        if self.runtime_output_format is not None:
            for fmt in self.output_and_config:
                if fmt.ext() == self.runtime_output_format.ext():
                    return fmt
            supported = ", ".join(sorted({fmt.ext() for fmt in self.output_and_config}))
            raise ValueError(
                f"Output type '{self.runtime_output_format.value}' "
                f"(extension '{self.runtime_output_format.ext()}') is not supported "
                f"by image '{self.name}'. Supported output extensions: {supported}."
            )
        return next(iter(self.output_and_config))

    def config_path(self) -> Path:
        # output_format() returns a key of output_and_config (or raises),
        # so index directly.
        return self.output_and_config[self.output_format()]

    def full_yaml_path(self) -> Path:
        return self.base_dir() / self.config_path()

    def dependencies(self) -> List[Path]:
        deps = [self.base_image.path]
        if not self.image_customizer_convert:
            deps.append(self.full_yaml_path())
        for file in self.base_dir().rglob("*"):
            if file.is_file():
                deps.append(file)
        self.base_dir().glob
        if self.requires_trident:
            deps.append(RpmSources.TRIDENT.path())
            deps.extend(RpmSources.TRIDENT.path().rglob("*.rpm"))
        if self.requires_dhcp:
            deps.append(RpmSources.DHCP.path())
            deps.extend(RpmSources.DHCP.path().rglob("*.rpm"))
        deps.extend(self.extra_dependencies)
        return deps

    def file_name(self) -> str:
        """
        Returns the file name for the image.
        """
        return f"{self.id}.{self.output_format().ext()}"

    def file_name_unsigned_raw(self) -> str:
        """Returns the file name for the unsigned raw image."""
        return f"{self.id}-unsigned.{OutputFormat.RAW.ext()}"

    def set_suffix(self, suffix: str) -> None:
        self.suffix = suffix

    @property
    def id(self) -> str:
        """Return the image ID."""
        if self.suffix is None:
            return self.name
        return f"{self.name}_{self.suffix}"

    def set_output_type(self, output_type: str) -> None:
        """Set the runtime output type based on a string."""
        try:
            self.runtime_output_format = OutputFormat(output_type)
            if output_type == OutputFormat.COSI.ext():
                log.warning(
                    "Output type 'cosi' was specified; if 'baremetal-image' was intended, use that as the output type."
                )
            if output_type == OutputFormat.VHD.ext():
                log.warning(
                    "Output type 'vhd' was specified; if 'vhd-fixed' was intended, use that as the output type."
                )
        except ValueError as e:
            valid_formats = ", ".join([fmt.value for fmt in OutputFormat])
            raise ValueError(
                f"Invalid output type '{output_type}'. Valid options are: {valid_formats}"
            ) from e

    def get_output_artifacts_dir(self) -> Optional[str]:
        """
        Return the output.artifacts.path from the image configuration YAML.

        Throws:
            ValueError if the path is present but empty.
        """
        path = self.base_ic_config.get("output", {}).get("artifacts", {}).get("path")
        if path is not None and not path:
            raise ValueError("output.artifacts.path cannot be empty")
        return path

    def get_items_to_sign(self) -> List[str]:
        """Return the list of items to sign from the image configuration YAML."""
        return (
            self.base_ic_config.get("output", {}).get("artifacts", {}).get("items", [])
        )


# IMPORTANT: THESE NAMES ARE EXPOSED IN THE CLI, MAKE SURE TO UPDATE ALL
# REFERENCES IF YOU CHANGE THEM!
@dataclass
class ArtifactManifest:
    customizer_version: str
    customizer_container: str
    customizer_container_full: str = None
    base_images: List[Union["BaseImageManifest", "BlobImageManifest"]] = field(
        default_factory=list
    )

    def __post_init__(self):
        if self.customizer_container_full is None:
            self.customizer_container_full = self.customizer_container
        if ":" not in self.customizer_container_full:
            self.customizer_container_full = (
                f"{self.customizer_container}:{self.customizer_version}"
            )

    @classmethod
    def kebab_fields(cls) -> List[str]:
        """Return a list of fields in kebab-case."""
        return [f.name.replace("_", "-") for f in fields(cls)]

    def find_base_image(
        self, img: BaseImage
    ) -> Optional[Union["BaseImageManifest", "BlobImageManifest"]]:
        """Find a base image by its name."""
        for base_image in self.base_images:
            if base_image.image == img:
                return base_image
        return None
