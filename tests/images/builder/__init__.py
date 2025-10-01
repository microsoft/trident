from dataclasses import dataclass, field, fields
from enum import Enum
from pathlib import Path
from typing import List, Optional


@dataclass
class BaseImageData:
    name: str
    path: Path


class BaseImage(Enum):
    BAREMETAL = BaseImageData("baremetal", Path("artifacts/baremetal.vhdx"))
    CORE_SELINUX = BaseImageData("core_selinux", Path("artifacts/core_selinux.vhdx"))
    QEMU_GUEST = BaseImageData("qemu_guest", Path("artifacts/qemu_guest.vhdx"))
    CORE_ARM64 = BaseImageData("core_arm64", Path("artifacts/core_arm64.vhdx"))
    MINIMAL = BaseImageData("minimal", Path("artifacts/minimal.vhdx"))

    @property
    def path(self) -> Path:
        return self.value.path

    @property
    def name(self) -> str:
        return self.value.name

    def __str__(self) -> str:
        return self.value.name


@dataclass
class BaseImageManifest:
    image: BaseImage
    package_name: str
    version: str
    org: str = "https://dev.azure.com/mariner-org/"
    project: str = "36d030d6-1d99-4ebd-878b-09af1f4f722f"
    feed: str = "AzureLinuxArtifacts"
    glob: str = "*.vhdx"


class OutputFormat(Enum):
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
        return self.value


class RpmSources(Enum):
    TRIDENT = Path("base/trident")
    DHCP = Path("base/dhcp")
    RPM_OVERRIDES = Path("base/rpm-overrides")

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

    # YAML config file inside the config dir
    config_file: Path = Path("base/baseimg.yaml")

    # The base image to use
    base_image: BaseImage = BaseImage.BAREMETAL

    # Whether the image requires Trident RPMs
    requires_trident: bool = True

    # Whether the image requires DHCP RPMs
    requires_dhcp: bool = False

    # Desired output format for this image
    output_format: OutputFormat = OutputFormat.COSI

    # Extra dependencies for this image
    extra_dependencies: List[Path] = field(default_factory=list)

    # Requires ukify to be present on the host
    requires_ukify: bool = False

    # When present, path to write a public SSH key to for customizer to consume
    # into the image. Both keys will be written to the output directory.
    ssh_key: Optional[Path] = None

    # Architecture of the image
    architecture: SystemArchitecture = SystemArchitecture.AMD64

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

        # Update config_file to be a Path object if it's a string
        if isinstance(self.config_file, str):
            self.config_file = Path(self.config_file)

        # Automatically set the architecture to arm64 if the base image is ARM64
        if self.base_image == BaseImage.CORE_ARM64:
            self.architecture = SystemArchitecture.ARM64

    def base_dir(self) -> Path:
        return Path(self.source) / self.config

    def full_yaml_path(self) -> Path:
        return self.base_dir() / self.config_file

    def dependencies(self) -> List[Path]:
        deps = [self.base_image.path, self.full_yaml_path()]
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

    def file_name(self, as_unsigned_raw: bool = False) -> str:
        """
        Returns the file name for the image, defaulting to the signed image with requested output
        format.
        """
        if as_unsigned_raw:
            return f"{self.id}-unsigned.{OutputFormat.RAW.ext()}"
        return f"{self.id}.{self.output_format.ext()}"

    def set_suffix(self, suffix: str) -> None:
        self.suffix = suffix

    @property
    def id(self) -> str:
        """Return the image ID."""
        if self.suffix is None:
            return self.name
        return f"{self.name}_{self.suffix}"


# IMPORTANT: THESE NAMES ARE EXPOSED IN THE CLI, MAKE SURE TO UPDATE ALL
# REFERENCES IF YOU CHANGE THEM!
@dataclass
class ArtifactManifest:
    customizer_version: str
    customizer_container: str
    customizer_container_full: str = None
    base_images: List[BaseImageManifest] = field(default_factory=list)

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

    def find_base_image(self, img: BaseImage) -> Optional[BaseImageManifest]:
        """Find a base image by its name."""
        for base_image in self.base_images:
            if base_image.image == img:
                return base_image
        return None
