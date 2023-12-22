# Copyright (c) Microsoft Corporation.

from assertpy import assert_that  # type: ignore

from ..node_interface import INode


class RunnerTool:
    def __init__(self, node: INode):
        self.node: INode = node

    def run(self, module_name, test_name=None, parallel: bool = False) -> None:
        cmd = f"tests/{module_name}"
        if not parallel:
            cmd += " --test-threads 1"
        if test_name:
            cmd += f" {test_name}"

        result = self.node.execute(cmd, no_info_log=False, sudo=True)
        assert_that(result.exit_code).is_equal_to(0)
