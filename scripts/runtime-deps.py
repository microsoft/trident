#!/usr/bin/env python3

# Basic script to find all runtime dependencies of the project.
#
# Known limitations:
# - Distinguishing between test and non-test dependencies is not perfect.

import argparse
import enum
import os
import re
import fnmatch
import sys
import logging
from typing import Dict, Generator, List, Literal, NamedTuple, Set, Tuple

logging.basicConfig(level=logging.INFO)

parser = argparse.ArgumentParser()
parser.add_argument(
    "-r",
    "--runtime",
    help="Only output runtime dependencies.",
    action="store_true",
)
parser.add_argument(
    "-s",
    "--summary",
    help="Only output the summary.",
    action="store_true",
)
parser.add_argument(
    "--resolve",
    help="Resolve dependencies to packages.",
    action="store_true",
)
parser.add_argument(
    "-c",
    "--cleanup-srpm",
    dest="cleanup_srpm",
    help="Report SRPM names as the base name without the version.",
    action="store_true",
)
parser.add_argument(
    "-j",
    "--json",
    help="Output as JSON.",
    action="store_true",
)
args = parser.parse_args()


class IsTest(str, enum.Enum):
    PROBABLY_NOT = "PROBABLY_NOT"
    PROBABLY = "PROBABLY"

    def report(self) -> str:
        return " (probably test dependency)" if bool(self) else ""

    def __bool__(self) -> Literal[True]:
        if self == IsTest.PROBABLY_NOT:
            return False
        elif self == IsTest.PROBABLY:
            return True
        else:
            raise ValueError("Invalid value for IsTest")


FileMatch = NamedTuple(
    "FileMatch", [("filename", str), ("line", int), ("cmd", str), ("is_test", IsTest)]
)


def find_matches(include_glob: str, match_pattern: str) -> List[FileMatch]:
    repo_root = d = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    regex = re.compile(match_pattern)
    output: List[re.Match] = []
    for root, dirnames, fnames in os.walk(repo_root):
        # Remove .git and target directories from search
        if root == repo_root:
            if ".git" in dirnames:
                dirnames.remove(".git")
            if "target" in dirnames:
                dirnames.remove("target")

        # Search for files matching include_glob
        for fname in fnames:
            if not fnmatch.fnmatch(fname, include_glob):
                continue

            # Open file and search for match_pattern
            full_path = os.path.join(root, fname)
            with open(full_path, "r") as f:
                data = f.read()

            # Check if test section is defined
            test_match = re.search(r"#\[cfg\(test\)\]\s*\nmod +tests?\s*{", data)
            test_pos = test_match.start() if test_match else None

            m_iter = regex.finditer(data)
            for m in m_iter:
                # Based on the position of the match, determine if it is a test or not
                if test_pos is not None and m.start() > test_pos:
                    # print(f"Found test dependency in {full_path}")
                    is_test = IsTest.PROBABLY
                else:
                    # print(f"Found non-test dependency in {full_path}")
                    is_test = IsTest.PROBABLY_NOT

                # Extract the line number
                lineno = m.string[: m.start()].count("\n") + 1
                # Get relative path
                rel_path = os.path.relpath(full_path, repo_root)
                # Add the match to the output
                output.append(FileMatch(rel_path, lineno, m.group(1), is_test))
    return output


matches: List[FileMatch] = []
matches.extend(find_matches("*.rs", r'Command::new\(\s*"([^"]+)"\s*\)'))
matches.extend(find_matches("*.rs", r'cmd!\(\s*"([^"]+)"\s*[),]'))

cmddict: Dict[str, IsTest] = dict()

for match in matches:
    # Only print if summary == False
    if not args.summary:
        print(
            f"{match.filename}:{match.line}:".ljust(50),
            f"{match.cmd.ljust(20)}{match.is_test.report()}",
            file=sys.stderr,
        )

    # If we already have a match, check if it is a test or not. If it appears it
    # is NOT a test dependency, then we can roll up all instances as a non-test
    # dependency.
    is_test = match.is_test
    if match.cmd in cmddict and cmddict[match.cmd] == IsTest.PROBABLY_NOT:
        is_test = IsTest.PROBABLY_NOT

    cmddict[match.cmd] = is_test

cmdlist: List[str] = sorted(cmddict.keys())

# Filter out test dependencies if --runtime is set
cmdlist = [cmd for cmd in cmdlist if not args.runtime or not cmddict[cmd]]


class Dependency:
    def __init__(self, cmd: str, package: str, srpm: str):
        self.cmd = cmd
        self.package = package
        self.srpm = srpm

    def set_package(self, package: str):
        self.package = package

    def set_srpm(self, srpm: str):
        if args.cleanup_srpm:
            self.srpm = srpm.split("-0:")[0]
        else:
            self.srpm = srpm


class Dependencies:
    def __init__(self, cmdlist: List[str]):
        self.dependencies = [Dependency(cmd, None, None) for cmd in cmdlist]

    def __iter__(self):
        return iter(self.dependencies)

    def print(self):
        if args.json:
            import json

            print(
                json.dumps(
                    [
                        {"cmd": dep.cmd, "package": dep.package, "srpm": dep.srpm}
                        for dep in self.dependencies
                    ],
                    indent=2,
                )
            )
        else:
            for dep in self.dependencies:
                if dep.package is None:
                    dep.package = "<unknown>"
                if dep.srpm is None:
                    dep.srpm = "<unknown>"
            ljust1 = max([len(dep.cmd) for dep in self.dependencies]) + 1
            ljust2 = max([len(dep.package) for dep in self.dependencies]) + 1
            print(f"{'Command'.ljust(ljust1)}{'Package'.ljust(ljust2)}{'SRPM'}")
            for dep in self.dependencies:
                print(f"{dep.cmd.ljust(ljust1)}{dep.package.ljust(ljust2)}{dep.srpm}")


dependencies = Dependencies(cmdlist)

if args.resolve:
    import contextlib
    import docker
    from docker.models.containers import Container

    client = docker.from_env()

    @contextlib.contextmanager
    def managed_container():
        logging.info("Starting container...")
        container: Container = client.containers.run(
            "mcr.microsoft.com/cbl-mariner/base/core:2.0",
            "bash -c 'while true; do sleep 20; done'",
            detach=True,
        )
        try:
            yield container
        finally:
            logging.info("Stopping container...")
            container.stop()
            logging.info("Removing container...")
            container.remove()

    def run_cmd(container: Container, cmd: str) -> str:
        logging.debug(f"Running '{cmd}'")
        out = container.exec_run(cmd, user="root")
        output = out.output.decode("utf-8").strip()
        if out.exit_code != 0:
            logging.debug(f"Failed to run '{cmd}'")
            raise Exception(f"Failed to run '{cmd}':\n{output}")
        return output

    def resolve_dependency(cmdlist: List[str]):
        with managed_container() as container:
            logging.info("Installing dnf...")
            run_cmd(container, "tdnf --nogpgcheck install -y dnf")
            # Dummy call for dnf to create the cache
            run_cmd(container, "dnf --nogpgcheck search dnf -y")

            for dep in dependencies:
                cmd = dep.cmd
                logging.info(f"Resolving {cmd}...")
                try:
                    out = run_cmd(container, f"dnf --nogpgcheck provides {cmd}")
                    lines = out.split("\n")
                    if len(lines) < 3 or "Error: No Matches found" in lines[-1]:
                        logging.error(f"No package found for {cmd}")
                        continue

                    dep.package = lines[-4].split()[0]
                    logging.info(f"Found package for {cmd}: {dep.package}")

                    out = run_cmd(
                        container, f"dnf --nogpgcheck repoquery --srpm -q {dep.package}"
                    )
                    lines = out.split("\n")
                    if len(lines) < 1:
                        logging.error(f"No srpm found for {dep.package}")
                        continue

                    srpm = lines[-1]
                    dep.srpm = srpm
                    logging.info(f"Found srpm for {dep.package}: {dep.srpm}")
                except Exception as e:
                    logging.error(f"Failed to resolve {cmd}: {e}")

    resolve_dependency(cmdlist)

dependencies.print()
