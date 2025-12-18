#! /usr/bin/env python3

import tomlkit
import json

from pathlib import Path
from typing import List, Dict, Set
from enum import Enum
from semver import Version
from tomlkit.items import Table

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
        elif isinstance(value, dict):
            self.version = value.get("version", "")
            self.features = value.get("features", None)
            self.options = {
                k: v for k, v in value.items() if k != "version" and k != "features"
            }

    def __eq__(self, value):
        return (
            self.name == value.name
            and self.version == value.version
            and self.options.get("features") == value.options.get("features")
        )

    def __hash__(self):
        return hash(
            (
                self.name,
                self.version,
                json.dumps(self.options.get("features"), sort_keys=True),
            )
        )

    def __repr__(self):
        return f"Dependency(name={self.name}, version={self.version}, features={self.features}, options={self.options})"

    def __lt__(self, other):
        if self.name != other.name:
            return self.name < other.name
        return Version.parse(self.version) < Version.parse(other.version)

    def __options(self) -> Dict[str, object]:
        return self.options if self.options is not None else {}

    def is_workspace(self) -> bool:
        return self.__options().get("workspace", False)

    def is_path(self) -> bool:
        return bool(self.__options().get("path"))

    def to_toml_root(self) -> object:
        if self.options:
            options_table = tomlkit.inline_table()
            options_table.add("version", self.version)
            for k, v in self.options.items():
                if k in ["optional"]:
                    continue
                options_table.add(k, v)
            return options_table
        else:
            return self.version


class CargoFile:
    def __init__(self, path: Path):
        self.path = path
        with open(path, "r") as f:
            self.toml = tomlkit.parse(f.read())

    def scan_dependencies(self, dep_type: DependencyType) -> List[Dependency]:
        return [
            Dependency(name, entry)
            for name, entry in self.toml.get(dep_type.value, {}).items()
        ]

    def set_dependencies_to_workspace(self, names: Set[str], dep_type: DependencyType):
        for name in names:
            if name in self.toml.get(dep_type.value, {}):
                self.toml[dep_type.value][name] = {"workspace": True}

    def save(self):
        with open(self.path, "w") as f:
            f.write(tomlkit.dumps(self.toml))


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

    def scan_dependencies(self, dep_type: DependencyType):
        dependencies: Dict[Dependency, int] = {}
        for cargo_file in self.cargo_files:
            deps = cargo_file.scan_dependencies(dep_type)
            for dep in deps:
                if dep.is_workspace() or dep.is_path():
                    continue
                dependencies.setdefault(dep, 0)
                dependencies[dep] += 1
        return dependencies

    def save_all(self):
        self.root.save()
        for cargo_file in self.cargo_files:
            cargo_file.save()

    def sync_dependencies(self, dep_type: DependencyType):
        dependencies = self.scan_dependencies(dep_type)
        # Filter dependencies that appear more than once
        dependencies = [k for k, v in dependencies.items() if v > 1]
        # Merge into root Cargo.toml
        self.merge_root_dependencies(dependencies, dep_type)
        # Update all Cargo.toml files to reference the root version
        for cargo_file in self.cargo_files:
            cargo_file.set_dependencies_to_workspace(
                {dep.name for dep in dependencies}, dep_type
            )

    def merge_root_dependencies(
        self, dependencies: List[Dependency], dep_type: DependencyType
    ):
        # For each dependency, make sure we have just one name, keeping the
        # latest version, and merging feature lists, also preserve
        # "default-features = false"

        # Create a dict to collect all collected dep names.
        deps: Dict[str, Dependency] = {}

        # Ingest all existing dependencies from root Cargo.toml and collect them
        # into the deps dict.
        for dep in self.root.scan_dependencies(dep_type):
            deps[dep.name] = dep

        # Now go
        for dep in dependencies:
            if dep.name not in deps:
                deps[dep.name] = dep
            else:
                # Keep the latest version
                existing_dep = deps[dep.name]
                latest_dep = max(existing_dep, dep)
                print(
                    f"Found multiple versions of {dep.name}: {existing_dep.version} and {dep.version}, keeping {latest_dep.version}"
                )
                deps[dep.name] = latest_dep
        dependencies = sorted(list(deps.values()))

        print(
            f"Merging {len(dependencies)} dependencies into {self.root.path.name} [{dep_type.value}]"
        )

        dep_table = self.root.toml.get(dep_type.value, tomlkit.table())
        for dep in dependencies:
            dep_table.add(dep.name, dep.to_toml_root())
        self.toml[dep_type.value] = dep_table


def main():
    repo = CargoRepository(TRIDENT_ROOT / "Cargo.toml")

    for dep_type in [DependencyType.REGULAR, DependencyType.DEV, DependencyType.BUILD]:
        repo.sync_dependencies(dep_type)

    repo.save_all()


if __name__ == "__main__":
    main()
