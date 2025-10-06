from pathlib import Path, PurePath
from typing import Dict, List, Optional, Protocol, Union

from assertpy.assertpy import AssertionBuilder  # type: ignore


class IExecutableResult(Protocol):
    stdout: str
    stderr: str
    exit_code: Optional[int]
    cmd: Union[str, List[str]]
    elapsed: float
    is_timeout: bool

    def assert_exit_code(
        self,
        expected_exit_code: Union[int, List[int]] = 0,
        message: str = "",
        include_output: bool = False,
    ) -> AssertionBuilder:
        pass

    def save_stdout_to_file(self, saved_path: Path) -> "IExecutableResult":
        pass


class IProcess(Protocol):
    def wait_result(
        self,
        timeout: float = 600,
        expected_exit_code: Optional[int] = None,
        expected_exit_code_failure_message: str = "",
    ) -> IExecutableResult:
        pass


class IShell(Protocol):
    def copy(self, local_path: PurePath, node_path: PurePath) -> None:
        pass

    def copy_back(self, node_path: PurePath, local_path: PurePath) -> None:
        pass

    def mkdir(
        self,
        path: PurePath,
        mode: int = 0o777,
        parents: bool = True,
        exist_ok: bool = False,
    ) -> None:
        pass


class INode(Protocol):
    name: str

    def execute(
        self,
        cmd: str,
        shell: bool = False,
        sudo: bool = False,
        nohup: bool = False,
        no_error_log: bool = False,
        no_info_log: bool = True,
        no_debug_log: bool = False,
        cwd: Optional[PurePath] = None,
        timeout: int = 600,
        update_envs: Optional[Dict[str, str]] = None,
        expected_exit_code: Optional[int] = None,
        expected_exit_code_failure_message: str = "",
    ) -> IExecutableResult:
        pass

    def execute_async(
        self,
        cmd: str,
        shell: bool = False,
        sudo: bool = False,
        nohup: bool = False,
        no_error_log: bool = False,
        no_info_log: bool = True,
        no_debug_log: bool = False,
        cwd: Optional[PurePath] = None,
        update_envs: Optional[Dict[str, str]] = None,
    ) -> IProcess:
        pass

    @property
    def working_path(self) -> PurePath:
        pass

    @property
    def local_working_path(self) -> Path:
        pass

    @property
    def local_log_path(self) -> Path:
        pass

    @property
    def shell(self) -> IShell:
        pass


class IRemoteNode(INode):
    public_address: str
    public_port: str
    internal_address: str
    internal_port: str
