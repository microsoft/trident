# Copyright (c) Microsoft Corporation.

from assertpy import assert_that  # type: ignore

from ..node_interface import INode


class TridentTool:
    def __init__(self, node: INode):
        self.node: INode = node

    def run(
        self,
    ) -> None:
        cmd = f"sudo trident run --verbosity DEBUG"

        result = self.node.execute(cmd)
        assert_that(result.exit_code).is_equal_to(0)

    def get(
        self,
    ) -> None:
        cmd = f"trident get --verbosity DEBUG"

        result = self.node.execute(cmd)
        assert_that(result.exit_code).is_equal_to(0)
        return result.stdout

    def start_network(
        self,
    ) -> None:
        cmd = f"sudo trident start-network --verbosity DEBUG"

        result = self.node.execute(cmd)
        assert_that(result.exit_code).is_equal_to(0)
