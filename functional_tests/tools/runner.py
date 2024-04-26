# Copyright (c) Microsoft Corporation.

import sys
from assertpy import assert_that  # type: ignore

from ..node_interface import INode


def format_exception_message(result):
    failures = result.stdout.split("failures:")
    if not failures or len(failures) < 2:
        return result.stdout

    errors = failures[1].split("\n")
    if not errors or len(errors) < 2:
        return result.stdout

    for e in errors:
        if e.startswith("thread "):
            return e

    return result.stdout


class RunnerTool:
    def __init__(self, node: INode):
        self.node: INode = node

    def run(self, module_name, test_name=None, parallel: bool = False) -> None:
        # For some reason, passing RUST_BACKTRACE=1 here works, while passing it
        # via the `update_envs` parameter in `execute()` does not.
        cmd = f"RUST_BACKTRACE=1 tests/{module_name}"
        if not parallel:
            cmd += " --test-threads 1"
        if test_name:
            cmd += f" {test_name}"

        result = self.node.execute(cmd, no_info_log=False, sudo=True)
        print(result.stdout)
        print(result.stderr, file=sys.stderr)
        if result.exit_code != 0:
            raise Exception(f"Test failed: {format_exception_message(result)}")
