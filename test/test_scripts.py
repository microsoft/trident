import importlib.util
import io
import json
from pathlib import Path
import socket
import tempfile
import threading
import time
import unittest

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("installer_vm", HERE / "run.py")
VM = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VM)


class VmCommandTests(unittest.TestCase):
    def test_follows_new_serial_records_without_replaying_firmware_escape_codes(self):
        class RunningProcess:
            def __init__(self):
                self.finished = threading.Event()

            def poll(self):
                return 0 if self.finished.is_set() else None

        process = RunningProcess()
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial.log"
            output = io.StringIO()

            def guest():
                time.sleep(0.03)
                with serial.open("w") as log:
                    log.write("\x1b[2JWelcome to firmware\r\n")
                    log.write("00:00 [INST:INFO] Installer starting\r\n")
                    log.flush()
                    time.sleep(0.03)
                    log.write("00:01 [TRIDENT:TRACE] first\r\n")
                    log.write("00:01 [TRIDENT:TRACE] second\r\n")
                    log.flush()
                    process.finished.set()

            writer = threading.Thread(target=guest)
            writer.start()
            self.assertEqual(
                VM.follow_serial(process, serial, output=output, interval=0.005), 0
            )
            writer.join(timeout=3)
            self.assertFalse(writer.is_alive())
            self.assertEqual(
                output.getvalue(),
                "00:00 [INST:INFO] Installer starting\n"
                "00:01 [TRIDENT:TRACE] first\n"
                "00:01 [TRIDENT:TRACE] second\n",
            )

    def test_vnc_is_headless_and_bound_to_loopback(self):
        launch = VM.command(
            Path("/tmp/installer-test.iso"),
            Path("/tmp/installer-test-owned"),
            Path("/usr/share/OVMF/OVMF_CODE_4M.fd"),
            6144,
            2,
            "gtk",
            vnc=1,
        )
        self.assertEqual(launch[launch.index("-display") + 1], "none")
        self.assertEqual(launch[launch.index("-vnc") + 1], "127.0.0.1:1")
        self.assertIn("-S", launch)
        self.assertEqual(
            launch[launch.index("-qmp") + 1],
            "unix:/tmp/installer-test-owned/monitor.sock,server=on,wait=off",
        )
        self.assertNotIn("gtk", launch)

    def test_vnc_connection_resumes_paused_guest_after_viewer_handshake(self):
        class RunningProcess:
            def poll(self):
                return None

        with tempfile.TemporaryDirectory() as directory:
            monitor = Path(directory) / "monitor.sock"
            commands = []
            ready = []

            def monitor_server():
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                    listener.bind(str(monitor))
                    listener.listen(1)
                    connection, _ = listener.accept()
                    with connection, connection.makefile("r") as reader:
                        connection.sendall(
                            b'{"QMP":{"version":{},"capabilities":[]}}\r\n'
                        )
                        for expected in ["qmp_capabilities", "query-vnc", "cont"]:
                            request = json.loads(reader.readline())
                            commands.append(request["execute"])
                            if expected == "query-vnc":
                                connection.sendall(
                                    b'{"return":{"enabled":true,"clients":[]},"id":"query-vnc"}\r\n'
                                )
                                connection.sendall(b'{"event":"VNC_INITIALIZED"}\r\n')
                            else:
                                response = {"return": {}, "id": expected}
                                connection.sendall(
                                    json.dumps(response).encode() + b"\r\n"
                                )

            server = threading.Thread(target=monitor_server, daemon=True)
            server.start()
            VM.wait_for_vnc_viewer(
                RunningProcess(), monitor, lambda: ready.append(True)
            )
            server.join(timeout=3)
            self.assertFalse(server.is_alive())
            self.assertEqual(commands, ["qmp_capabilities", "query-vnc", "cont"])
            self.assertEqual(ready, [True])

    def test_vm_exposes_only_new_disposable_storage(self):
        scratch = Path("/tmp/installer-test-owned")
        launch = VM.command(
            Path("/tmp/installer-test.iso"),
            scratch,
            Path("/usr/share/OVMF/OVMF_CODE_4M.fd"),
            6144,
            2,
            "gtk",
        )
        drives = [
            launch[index + 1] for index, value in enumerate(launch) if value == "-drive"
        ]
        self.assertEqual(len(drives), 4)
        self.assertIn(f"file={scratch}/disk.qcow2", drives[2])
        self.assertIn("readonly=on", drives[0])
        self.assertIn("readonly=on", drives[3])
        self.assertNotIn("-virtfs", launch)
        self.assertNotIn("-fsdev", launch)
        self.assertIn("virtio-blk-pci,drive=hd0,bootindex=1", launch)
        self.assertIn("ide-cd,bus=ide.0,drive=cd0,bootindex=2", launch)
        self.assertIn("gtk", launch)
        self.assertNotIn("-vnc", launch)
        self.assertEqual(
            launch[launch.index("-serial") + 1], f"file:{scratch}/serial.log"
        )


if __name__ == "__main__":
    unittest.main()
