#!/usr/bin/env python3

# Basic script to find all runtime dependencies of the project.
#
# Known limitations:
# - Distinguishing between test and non-test dependencies is not perfect.

import enum
import os
import re
import fnmatch
from typing import Dict, List, NamedTuple, Set, Tuple


class IsTest(str, enum.Enum):
    PROBABLY_NOT = "PROBABLY_NOT"
    PROBABLY = "PROBABLY"

    def report(self) -> str:
        if self == IsTest.PROBABLY_NOT:
            return ""
        elif self == IsTest.PROBABLY:
            return " (probably test dependency)"
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
    print(
        f"{match.filename}:{match.line}:".ljust(50),
        f"{match.cmd.ljust(20)}{match.is_test.report()}",
    )

    # If we already have a match, check if it is a test or not If it appears it
    # is NOT a test dependency, then we can roll up all instances as a non-test
    # dependency
    is_test = match.is_test
    if match.cmd in cmddict and cmddict[match.cmd] == IsTest.PROBABLY_NOT:
        is_test = IsTest.PROBABLY_NOT

    cmddict[match.cmd] = is_test

cmdlist: List[str] = sorted(cmddict.keys())

print()
print("Binary dependencies:")
for cmd in cmdlist:
    print(f"  {cmd.ljust(20)}{cmddict[cmd].report()}")
