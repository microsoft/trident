from pathlib import Path
from typing import Dict, Optional
import enum
import json
from typing import Any, List, NamedTuple


class ConfigFlags(enum.IntFlag):
    NONE = 0
    EMULATED_TPM = enum.auto()

    def __str__(self) -> str:
        return self.to_str()

    @staticmethod
    def flag_dict() -> Dict[str, "ConfigFlags"]:
        return {
            "b": ConfigFlags.NONE,
            "t": ConfigFlags.EMULATED_TPM,
        }

    def to_str(self) -> str:
        """Converts a ConfigFlags enum value to a single character"""
        reverse_dict = {v: k for k, v in ConfigFlags.flag_dict().items()}
        return reverse_dict[ConfigFlags.NONE] + "".join(
            [reverse_dict[flag] for flag in ConfigFlags if self in flag]
        )

    @staticmethod
    def from_char(char: str) -> "ConfigFlags":
        """Converts a single character to a ConfigFlags enum value"""
        try:
            return ConfigFlags.flag_dict()[char]
        except KeyError:
            raise ValueError(f"Unknown flag: {char}")

    @staticmethod
    def from_str(
        val: str,
        default: Optional["ConfigFlags"] = None,
        base: Optional["ConfigFlags"] = None,
    ) -> "ConfigFlags":
        """Converts a string of 0..n flags to a ConfigFlags value
        If default is given, it will be used as a fallback if no flags are given.
        If base is given, it will be used as a base for the flags.
        """
        if val == "":
            return default or ConfigFlags.NONE
        if base is None:
            base = ConfigFlags.NONE

        flags = base
        for char in val:
            flags |= ConfigFlags.from_char(char)
        return flags


class CloudInitConfig(NamedTuple):
    userdata: Path
    metadata: Path


class VirtualMachineTemplate(NamedTuple):
    name: str = "MISSING NAME!"
    flags: ConfigFlags = ConfigFlags.NONE
    cpus: int = 4
    mem: int = 2  # In GiB
    disks: List[int] = [16]  # In GiB
    os_disk: Optional[Path] = None
    cloud_init: Optional[CloudInitConfig] = None


class DeploymentTemplate:
    @staticmethod
    def fromJson(config: str) -> "DeploymentTemplate":
        data = json.loads(config)

    def __init__(self, vmtemplates: List[VirtualMachineTemplate]) -> None:
        self._templates = vmtemplates

    def getTemplates(self) -> List[VirtualMachineTemplate]:
        return self._templates

    def toJson(self) -> str:
        data = [item._asdict() for item in self._templates]
        return json.dumps(data, indent=4)
