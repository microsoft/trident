#! /usr/bin/env python3

"""
scripts/sync-rust-deps.py

Synchronize Rust dependencies across all Cargo.toml files in the workspace by
merging versions and features into the root Cargo.toml workspace section, and
updating all member Cargo.toml files to reference the root versions.

When multiple versions of the same dependency are found, the latest version is
kept, and features are merged.

When workspace cargo files have different settings for `features` or
`default-features`, those settings are preserved when switching to workspace
dependencies.

All other settings are preserved as well, except for `version`, which is always
taken from the root Cargo.toml.
"""

import tomlkit
import json
import copy

from pathlib import Path
from typing import List, Dict, Optional, Set
from enum import Enum
from semver import Version
from dataclasses import dataclass

TRIDENT_ROOT = Path(__file__).parent.parent


class DependencyType(Enum):
    REGULAR = "dependencies"
    DEV = "dev-dependencies"
    BUILD = "build-dependencies"


class Dependency:
    def __init__(self, name: str, value: object):
        self.name = name

        if isinstance(value, str):
            self.version = value
            self.options = {}
            self.features = None
            self.default_features = None
        elif isinstance(value, dict):
            self.version = value.get("version", "")
            self.features = value.get("features", None)
            self.default_features = value.get("default-features", None)
            self.options = {
                k: v
                for k, v in value.items()
                if k != "version" and k != "features" and k != "default-features"
            }
        else:
            raise ValueError("Invalid dependency value type: " + str(type(value)))
        try:
            self.semver = Version.parse(self.version, optional_minor_and_patch=True)
        except ValueError:
            # Internal dependencies don't have a version we can parse. Default to 0.0.0
            self.semver = Version(0, 0, 0)

    def __eq__(self, value):
        return (
            self.name == value.name
            and self.version == value.version
            and self.feature_set() == value.feature_set()
        )

    def __hash__(self):
        return hash(
            (
                self.name,
                self.version,
                json.dumps(
                    sorted(self.feature_set()),
                    sort_keys=True,
                ),
            )
        )

    def __repr__(self):
        return f"Dependency(name={self.name}, version={self.version}, features={self.features}, options={self.options})"

    def __lt__(self, other):
        if self.name != other.name:
            return self.name < other.name
        return self.semver < other.semver

    def __options(self) -> Dict[str, object]:
        return self.options if self.options is not None else {}

    def is_workspace(self) -> bool:
        return self.__options().get("workspace", False)

    def is_path(self) -> bool:
        return bool(self.__options().get("path"))

    def feature_set(self) -> Set[str]:
        return set(self.features) if self.features is not None else set()

    def to_toml_root(self) -> object:
        values = {
            "version": self.version,
        }
        if self.features is not None:
            values["features"] = self.features
        if self.default_features is not None:
            values["default-features"] = self.default_features

        if len(values) > 1:
            options_table = tomlkit.inline_table()
            for k, v in values.items():
                options_table.add(k, v)
            return options_table
        else:
            return self.version


class CargoFile:
    def __init__(self, path: Path):
        self.path = path
        with open(path, "r") as f:
            self.toml = tomlkit.parse(f.read())

    def scan_dependencies(
        self, dep_type: DependencyType, is_workspace: bool = False
    ) -> List[Dependency]:
        toml = (
            self.toml
            if not is_workspace
            else self.toml.setdefault("workspace", tomlkit.table())
        )
        return [
            Dependency(name, entry)
            for name, entry in toml.get(dep_type.value, {}).items()
        ]

    def set_dependencies_to_workspace(
        self, dependencies: List[Dependency], dep_type: DependencyType
    ):
        for dep in dependencies:
            entry = self.toml.get(dep_type.value, {}).get(dep.name, None)
            if entry is None:
                continue
            print(f"Setting {dep.name} to workspace in {self.path}")
            new_entry = tomlkit.inline_table()
            new_entry.add("workspace", True)
            if isinstance(entry, str):
                # All done :)
                pass
            else:
                # Preserve features if they differ from root
                if features := entry.get("features", None):
                    curr_feature_set = set(features)
                    if curr_feature_set != dep.feature_set():
                        new_entry["features"] = features

                # Preserve default-features if set and differs from root
                if default_features := entry.get("default-features", None):
                    if dep.default_features != default_features:
                        new_entry["default-features"] = default_features

                # Preserve everything else
                skip = set(["workspace", "version", "features", "default-features"])
                for k, v in entry.items():
                    if k in skip:
                        continue
                    new_entry.add(k, v)
            self.toml[dep_type.value][dep.name] = new_entry

    def save(self):
        with open(self.path, "w") as f:
            f.write(tomlkit.dumps(self.toml))


def merge_dependency_options(dependencies: List[Dependency]) -> Dependency:
    features: Set[str] = set()
    default_features: Optional[bool] = None

    for dep in dependencies:
        if dep.features:
            features.update(dep.features)
        if dep.default_features is not None and dep.default_features is False:
            default_features = False

    latest = dependencies[0]
    for dep in dependencies[1:]:
        if dep.semver > latest.semver:
            latest = dep
    merged = copy.deepcopy(latest)
    if features:
        merged.features = sorted(list(features))
    if default_features is not None:
        merged.default_features = default_features

    print(f"Merged dependency {merged.name} to version {merged.version}", end="")
    if merged.features:
        print(f" with features {merged.features}", end="")
    if default_features is not None:
        print(f" and default-features={merged.default_features}", end="")
    print("")
    return merged


class CargoRepository:
    def __init__(self, root: Path):
        self.root = CargoFile(root)
        self.cargo_files = self._load_cargo_files()

    def _load_cargo_files(self) -> List[CargoFile]:
        cargo_files = []
        workspace_members = self.root.toml.get("workspace", {}).get("members", [])
        for member in workspace_members:
            member_path = self.root.path.parent / member / "Cargo.toml"
            cargo_files.append(CargoFile(member_path))
        return cargo_files

    def scan_dependencies(
        self, dep_type: DependencyType
    ) -> Dict[str, List[Dependency]]:
        dependencies: Dict[str, List[Dependency]] = {}
        for cargo_file in self.cargo_files:
            deps = cargo_file.scan_dependencies(dep_type)
            for dep in deps:
                if dep.is_workspace() or dep.is_path():
                    continue
                dependencies.setdefault(dep.name, [])
                dependencies[dep.name].append(dep)
        return dependencies

    def save_all(self):
        self.root.save()
        for cargo_file in self.cargo_files:
            cargo_file.save()

    def sync_dependencies(self, dep_type: DependencyType):
        dependencies = self.scan_dependencies(dep_type)
        # Merge into root Cargo.toml
        root_deps = self.merge_root_dependencies(dependencies)
        # Update all Cargo.toml files to reference the root version
        for cargo_file in self.cargo_files:
            cargo_file.set_dependencies_to_workspace(root_deps, dep_type)

    def merge_root_dependencies(
        self,
        dependencies: Dict[str, List[Dependency]],
    ) -> List[Dependency]:
        # The root Cargo.toml only supports regular dependencies, all workspaces
        # pull from this list, regardless of the dependency type.
        dep_type = DependencyType.REGULAR

        # For each dependency, make sure we have just one entry per name:
        dependencies: Dict[str, Dependency] = {
            k: merge_dependency_options(v) for k, v in dependencies.items()
        }

        # Create a dict to collect all dep names. Fill it up with
        # existing deps from root Cargo.toml
        deps: Dict[str, Dependency] = {
            dep.name: dep
            for dep in self.root.scan_dependencies(dep_type, is_workspace=True)
        }

        # Now go over the collected dependencies and add them to the deps dict,
        # keeping only the latest version if multiple versions are found.
        for dep in dependencies.values():
            if dep.name not in deps:
                deps[dep.name] = dep
            else:
                # Keep the latest version
                existing_dep = deps[dep.name]
                keeping = merge_dependency_options([existing_dep, dep])
                print(
                    f"Found multiple versions of {dep.name}: {existing_dep.version} and {dep.version}, keeping {keeping.version}"
                )
                deps[dep.name] = keeping
        dependencies: List[Dependency] = sorted(list(deps.values()))

        print(
            f"Merging {len(dependencies)} dependencies into {self.root.path.name} [{dep_type.value}]"
        )

        dep_table = self.root.toml.setdefault("workspace", tomlkit.table()).setdefault(
            dep_type.value, tomlkit.table()
        )
        for dep in dependencies:
            if dep.name in dep_table:
                dep_table[dep.name] = dep.to_toml_root()
            else:
                dep_table.append(dep.name, dep.to_toml_root())

        return dependencies


def main():
    repo = CargoRepository(TRIDENT_ROOT / "Cargo.toml")

    for dep_type in [DependencyType.REGULAR, DependencyType.DEV, DependencyType.BUILD]:
        repo.sync_dependencies(dep_type)

    repo.save_all()


if __name__ == "__main__":
    main()
