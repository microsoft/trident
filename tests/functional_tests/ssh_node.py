import logging
import shlex
import time
from io import StringIO
from pathlib import Path, PurePath
from threading import Thread
from typing import Any, Dict, List, Optional, Union

from assertpy.assertpy import AssertionBuilder, assert_that  # type: ignore
from paramiko import SSHClient
from paramiko.channel import ChannelFile, ChannelStderrFile


class SshExecutableResult:
    def __init__(
        self,
        stdout: str,
        stderr: str,
        exit_code: Optional[int],
        cmd: Union[str, List[str]],
        elapsed: float,
        is_timeout: bool,
    ) -> None:
        self.stdout = stdout
        self.stderr = stderr
        self.exit_code = exit_code
        self.cmd = cmd
        self.elapsed = elapsed
        self.is_timeout = is_timeout

    def assert_exit_code(
        self,
        expected_exit_code: Union[int, List[int]] = 0,
        message: str = "",
        include_output: bool = False,
    ) -> AssertionBuilder:
        if isinstance(expected_exit_code, int):
            expected_exit_code = [expected_exit_code]

        assert isinstance(expected_exit_code, list)

        description_parts = []
        if message:
            description_parts.append(message)

        description_parts.append(f"unexpected exit code on: {self.cmd}")

        if include_output:
            description_parts.append("stdout:")
            description_parts.append(self.stdout)
            description_parts.append("stderr:")
            description_parts.append(self.stderr)

        description = "\n".join(description_parts)
        return assert_that(expected_exit_code, description).contains(self.exit_code)

    def save_stdout_to_file(self, saved_path: Path) -> "SshExecutableResult":
        with open(saved_path, "w") as f:
            f.write(self.stdout)
        return self


class _SshChannelFileReader:
    def __init__(
        self, channel_file: ChannelFile, log_level: int, log_name: str
    ) -> None:
        self._channel_file = channel_file
        self._log_level = log_level
        self._log_name = log_name
        self._output: Optional[str] = None

        self._thread: Thread = Thread(target=self._read_thread)
        self._thread.start()

    def close(self) -> None:
        self._thread.join()

    def __enter__(self) -> "_SshChannelFileReader":
        return self

    def __exit__(self, exc_type: Any, exc_value: Any, traceback: Any) -> None:
        self.close()

    def wait_for_output(self) -> str:
        self._thread.join()

        assert self._output is not None
        return self._output

    def _read_thread(self) -> None:
        log_enabled = logging.getLogger().isEnabledFor(self._log_level)

        with StringIO() as output:
            while True:
                # Read output one list at a time.
                line = self._channel_file.readline()
                if not line:
                    break

                # Store the line.
                output.write(line)

                # Log the line.
                if log_enabled:
                    logging.log(
                        self._log_level,
                        "%s: %s",
                        self._log_name,
                        line[:-1] if line.endswith("\n") else line,
                    )

            self._channel_file.close()
            self._output = output.getvalue()


class SshProcess:
    def __init__(
        self,
        cmd: str,
        stdout: ChannelFile,
        stderr: ChannelStderrFile,
        stdout_log_level: int,
        stderr_log_level: int,
    ) -> None:
        self.cmd = cmd
        self._channel = stdout.channel
        self._result: Optional[SshExecutableResult] = None

        self._start_time = time.monotonic()

        chanid = self._channel.chanid

        logging.debug("[ssh][%d][cmd]: %s", chanid, cmd)

        self._stdout_reader = _SshChannelFileReader(
            stdout, stdout_log_level, f"[ssh][{chanid}][stdout]"
        )
        self._stderr_reader = _SshChannelFileReader(
            stderr, stderr_log_level, f"[ssh][{chanid}][stderr]"
        )

    def close(self) -> None:
        self._channel.close()
        self._stdout_reader.close()
        self._stderr_reader.close()

    def __enter__(self) -> "SshProcess":
        return self

    def __exit__(self, exc_type: Any, exc_value: Any, traceback: Any) -> None:
        self.close()

    def wait_result(
        self,
        timeout: float = 600,
        expected_exit_code: Optional[int] = None,
        expected_exit_code_failure_message: str = "",
    ) -> SshExecutableResult:
        result = self._result
        if result is None:
            # Wait for the process to exit.
            completed = self._channel.status_event.wait(timeout)

            if completed:
                exit_code = self._channel.recv_exit_status()

            else:
                # Close channel.
                self._channel.close()

                # Set exit code to 1 to match LISA's behavior.
                exit_code = 1

            # Get the process's output.
            stdout = self._stdout_reader.wait_for_output()
            stderr = self._stderr_reader.wait_for_output()

            elapsed_time = time.monotonic() - self._start_time

            logging.debug(
                "[ssh][%d][cmd]: execution time: %f, exit code: %d",
                self._channel.chanid,
                elapsed_time,
                exit_code,
            )

            result = SshExecutableResult(
                stdout, stderr, exit_code, self.cmd, elapsed_time, not completed
            )
            self._result = result

        if expected_exit_code is not None:
            result.assert_exit_code(
                expected_exit_code=expected_exit_code,
                message=expected_exit_code_failure_message,
            )

        return result


class SshNode:
    def __init__(
        self,
        local_working_path: Path,
        local_log_path: Path,
        hostname: str,
        port: int = 22,
        username: Optional[str] = None,
        key_path: Optional[Path] = None,
        gateway: "Optional[SshNode]" = None,
        name: Optional[str] = None,
        working_path_subdir: Optional[str] = None,
        known_hosts_path: Optional[Path] = None,
    ) -> None:
        self.ssh_client: SSHClient
        self.name: str
        self._local_working_path: Path = local_working_path
        self._local_log_path: Path = local_log_path
        self._working_path_subdir: Optional[str] = working_path_subdir
        self._working_path: Optional[PurePath] = None

        sock = None
        if gateway:
            gateway_transport = gateway.ssh_client.get_transport()
            assert gateway_transport
            sock = gateway_transport.open_channel(
                "direct-tcpip", (hostname, port), ("", 0)
            )

        self.ssh_client = SSHClient()
        if known_hosts_path:
            self.ssh_client.load_host_keys(str(known_hosts_path))
        else:
            self.ssh_client.load_system_host_keys()
        key_filename = None if key_path is None else str(key_path.absolute())
        self.ssh_client.connect(
            hostname=hostname,
            port=port,
            username=username,
            key_filename=key_filename,
            sock=sock,
        )

        if name:
            self.name = name
        else:
            self.name = self.execute("hostname", expected_exit_code=0).stdout

    def close(self) -> None:
        self.ssh_client.close()

    def __enter__(self) -> "SshNode":
        return self

    def __exit__(self, exc_type: Any, exc_value: Any, traceback: Any) -> None:
        self.close()

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
    ) -> SshExecutableResult:
        with self.execute_async(
            cmd,
            shell=shell,
            sudo=sudo,
            nohup=nohup,
            no_error_log=no_error_log,
            no_info_log=no_info_log,
            no_debug_log=no_debug_log,
            cwd=cwd,
            update_envs=update_envs,
        ) as process:
            return process.wait_result(
                timeout=timeout,
                expected_exit_code=expected_exit_code,
                expected_exit_code_failure_message=expected_exit_code_failure_message,
            )

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
    ) -> SshProcess:
        if not shell:
            # SSH runs all commands in shell sessions.
            # So, to remove shell symantics, use shlex to escape all the shell symbols.
            cmd = shlex.join(shlex.split(cmd))

        if nohup:
            cmd = f"nohup {cmd}"

        if sudo:
            cmd = f"sudo {cmd}"

        if cwd is not None:
            cmd = f"cd {shlex.quote(str(cwd))}; {cmd}"

        stdout_log_level = logging.INFO
        if no_debug_log:
            stdout_log_level = logging.NOTSET
        elif no_info_log:
            stdout_log_level = logging.DEBUG

        stderr_log_level = logging.ERROR
        if no_error_log:
            stderr_log_level = stdout_log_level

        stdin, stdout, stderr = self.ssh_client.exec_command(
            cmd, environment=update_envs
        )
        stdin.close()

        return SshProcess(cmd, stdout, stderr, stdout_log_level, stderr_log_level)

    @property
    def working_path(self) -> PurePath:
        working_path = self._working_path
        if not working_path:
            home_path = self.execute(
                'echo "$HOME"', shell=True, expected_exit_code=0
            ).stdout
            working_path = PurePath(home_path)

            if self._working_path_subdir:
                working_path = working_path / self._working_path_subdir
                self.mkdir(working_path, parents=True, exist_ok=True)

            self._working_path = working_path

        return working_path

    @property
    def local_working_path(self) -> Path:
        return self._local_working_path

    @property
    def local_log_path(self) -> Path:
        return self._local_log_path

    @property
    def shell(self) -> "SshNode":
        return self

    def copy(self, local_path: PurePath, node_path: PurePath) -> None:
        with self.ssh_client.open_sftp() as sftp:
            sftp.put(str(local_path), str(node_path))

    def copy_back(self, node_path: PurePath, local_path: PurePath) -> None:
        with self.ssh_client.open_sftp() as sftp:
            sftp.get(str(node_path), str(local_path))

    def mkdir(
        self,
        path: PurePath,
        mode: int = 0o777,
        parents: bool = True,
        exist_ok: bool = False,
    ) -> None:
        if parents or exist_ok:
            self.execute(
                f"mkdir -m={mode:o} -p {shlex.quote(str(path))}",
                shell=True,
                expected_exit_code=0,
            )

        else:
            self.execute(
                f"mkdir -m={mode:o} {shlex.quote(str(path))}",
                shell=True,
                expected_exit_code=0,
            )
