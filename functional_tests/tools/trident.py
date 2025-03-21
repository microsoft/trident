# Copyright (c) Microsoft Corporation.

from assertpy import assert_that  # type: ignore

from ..node_interface import INode
from ..ssh_node import SshExecutableResult


class TridentTool:
    def __init__(self, node: INode):
        self.node: INode = node

    def commit(
        self,
        sudo=True,
    ) -> SshExecutableResult:
        cmd = f"trident commit --verbosity DEBUG"

        if sudo:
            cmd = "sudo " + cmd

        return self.node.execute(cmd)

    def get(self) -> str:
        result = self.node.execute(f"trident get --verbosity DEBUG")
        assert_that(result.exit_code).is_equal_to(0)
        return result.stdout

    def start_network(
        self,
    ) -> None:
        cmd = f"sudo trident start-network --verbosity DEBUG"

        result = self.node.execute(cmd)
        assert_that(result.exit_code).is_equal_to(0)

    def offline_initialize(
        self,
        host_status_path: str,
    ) -> None:
        cmd = f"sudo trident offline-initialize {host_status_path} --verbosity DEBUG"

        result = self.node.execute(cmd)
        assert_that(result.exit_code).is_equal_to(0)
