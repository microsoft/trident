import json
import os
import signal
import socket
import subprocess
import sys
import time
import unittest

from pathlib import Path
from unittest.mock import patch

import test_run_vm_e2e_lifecycle as fixtures

runner = fixtures.runner

DAEMON = r"""
import json, pathlib, socket, sys
path, mode = sys.argv[1:]
server = socket.socket(socket.AF_UNIX)
server.bind(path)
server.listen()
server.settimeout(.01)
pending = None
active = None
removed = []
queries = 0
mutations = 0
while True:
    if active:
        mutations += 1
        pathlib.Path(path + '.mutating-artifact').write_text(str(mutations))
    try:
        connection, _ = server.accept()
    except TimeoutError:
        continue
    with connection:
        request = json.loads(connection.makefile().readline())
        op = request.get('op')
        response = {'status': 0, 'stdout': ''}
        if op == 'shutdown':
            connection.sendall(json.dumps(response).encode() + b'\n')
            break
        if op == 'create':
            pending = {'Id': 'c' * 64, 'Name': '/' + request['name'],
                       'Owner': request['name'] if mode != 'wrong-owner' else 'not-owned'}
            if mode != 'late':
                active = pending
        elif op == 'release':
            active = pending
        elif op == 'state':
            response.update(active=active, removed=removed, queries=queries,
                            accepted=pending is not None, mutations=mutations)
        else:
            args = request['argv']
            if args[:2] == ['container', 'ls']:
                queries += 1
                assert args[args.index('--filter') + 1] == 'name=^/' + request['name'] + '$'
                response['stdout'] = active['Id'] + '\n' if active else ''
            elif args[:2] == ['container', 'inspect']:
                assert args[2] == active['Id']
                assert '.Config.Labels' in args[-1] and '.Env' not in args[-1]
                response['stdout'] = json.dumps(active)
            elif args[:3] == ['container', 'rm', '--force']:
                assert args[3] == active['Id']
                if mode == 'remove-failure':
                    response['status'] = 19
                else:
                    removed.append(args[3])
                    active = None
            else:
                raise AssertionError(args)
        connection.sendall(json.dumps(response).encode() + b'\n')
server.close()
"""

CLIENT = r"""
import json, socket, sys
with socket.socket(socket.AF_UNIX) as connection:
    connection.settimeout(2)
    connection.connect(sys.argv[1])
    connection.sendall(json.dumps({'argv': sys.argv[3:], 'name': sys.argv[2]}).encode() + b'\n')
    response = json.loads(connection.makefile().readline())
sys.stdout.write(response['stdout'])
sys.exit(response['status'])
"""

STORM = r"""#!/usr/bin/env python3
import json, pathlib, socket, sys, time
root = pathlib.Path.cwd()
if sys.argv[1] == 'script':
    intent = json.loads((root.parent / 'container-intent.json').read_text())
    assert not intent['createMayHaveBeenAccepted']
    for index in range(1, 4):
        (root / f'test-sysext-{index}.raw').write_text('owned mock sysext')
elif '--test-case-to-run' in sys.argv:
    (root / 'cleanup-invoked').write_text('owned domain only')
else:
    intent = json.loads((root / 'container-intent.json').read_text())
    assert intent['createMayHaveBeenAccepted']
    name = sys.argv[sys.argv.index('--image-customizer-container-name') + 1]
    assert intent['name'] == name
    with socket.socket(socket.AF_UNIX) as connection:
        connection.connect(SOCKET_PATH)
        connection.sendall(json.dumps({'op': 'create', 'name': name}).encode() + b'\n')
        assert json.loads(connection.makefile().readline())['status'] == 0
    (root / 'accepted-create').touch()
    while True:
        time.sleep(.05)
"""


@unittest.skipUnless(
    sys.platform == "linux", "requires native Unix sockets and process groups"
)
class ContainerBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.LifecycleRunnerTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.root = self.fixture.root
        self.args = self.fixture.args
        self.args.lane = "rollback-uki"
        self.args.timeout_seconds = 0.8
        self.args.cleanup_grace_seconds = 0.5
        self.socket = self.root / "daemon.sock"
        self.daemon_file = self.root / "daemon.py"
        self.daemon_file.write_text(DAEMON)
        self.client_file = self.root / "client.py"
        self.client_file.write_text(CLIENT)
        executable = STORM.replace("SOCKET_PATH", repr(str(self.socket))).encode()
        self.fixture.add_file(runner.TOOL_PATH, "lifecycle-go-tools", executable)
        self.args.tool_sha256 = self.fixture.manifest["files"][runner.TOOL_PATH][
            "sha256"
        ]
        self.fixture.manifest["lifecycle"]["toolSha256"] = self.args.tool_sha256
        self.fixture.save_manifest()

    def rpc(self, **request):
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(2)
            connection.connect(str(self.socket))
            connection.sendall(json.dumps(request).encode() + b"\n")
            return json.loads(connection.makefile().readline())

    def stop_daemon(self):
        try:
            if self.daemon.poll() is None:
                self.rpc(op="shutdown")
            self.daemon.wait(timeout=3)
        finally:
            if self.daemon.poll() is None:
                self.daemon.kill()
                self.daemon.wait(timeout=3)

    def execute(self, mode, interrupt=False):
        self.daemon = subprocess.Popen(
            [sys.executable, str(self.daemon_file), str(self.socket), mode],
            start_new_session=True,
        )
        self.addCleanup(self.stop_daemon)
        deadline = time.monotonic() + 3
        while not self.socket.exists():
            self.assertIsNone(self.daemon.poll())
            self.assertLess(time.monotonic(), deadline)
            time.sleep(0.01)
        cleanup_queries = []

        def commands(argv, cwd, log, timeout, grace):
            self.fixture.assert_locked()
            if argv[0] == "docker":
                intent = json.loads((cwd / "container-intent.json").read_text())
                if log.name != "container-preflight.log":
                    self.assertEqual(signal.getsignal(signal.SIGINT), signal.SIG_IGN)
                    self.assertEqual(signal.getsignal(signal.SIGTERM), signal.SIG_IGN)
                    signal.raise_signal(signal.SIGINT)
                    signal.raise_signal(signal.SIGTERM)
                    cleanup_queries.append(log.name)
                argv = [
                    sys.executable,
                    str(self.client_file),
                    str(self.socket),
                    intent["name"],
                    *argv[1:],
                ]
            if interrupt and "--image-customizer-container-name" in argv:
                # Interrupt the real shared command only after daemon acceptance.
                import threading

                def cancel():
                    accepted = cwd / "accepted-create"
                    end = time.monotonic() + 3
                    while not accepted.exists() and time.monotonic() < end:
                        time.sleep(0.01)
                    if accepted.exists():
                        os.kill(os.getpid(), signal.SIGINT)

                thread = threading.Thread(target=cancel)
                thread.start()
                try:
                    return fixtures.COMMAND(argv, cwd, log, 4, grace)
                finally:
                    thread.join(timeout=4)
                    self.assertFalse(thread.is_alive())
            return fixtures.COMMAND(argv, cwd, log, timeout, grace)

        with (
            patch.object(runner.shared, "HOST_LOCK", self.fixture.lock),
            patch.object(runner.shared, "command", side_effect=commands),
            patch.object(
                runner, "preflight", return_value=(self.fixture.key, self.fixture.qemu)
            ),
            patch.object(runner, "domain_absent"),
            patch.object(runner, "check_ports"),
        ):
            status = runner.execute(self.args)
        result = json.loads((self.args.output / "results.json").read_text())
        row = result["lanes"][3]
        self.assertEqual(status, 130 if interrupt else 124)
        self.assertEqual(row["suiteExitCode"], status)
        self.assertEqual(result["executed"], 1)
        self.assertTrue(result["requiresHostDisposal"])
        self.assertEqual(row["container"]["outcome"], "uncertain")
        self.assertTrue((self.args.output / "rollback-uki/cleanup-invoked").is_file())
        self.assertIsNone(
            self.daemon.poll(), "Storm group teardown killed the independent daemon"
        )
        self.assertIn("container-before-cleanup.log", cleanup_queries)
        return result, self.rpc(op="state")

    def test_daemon_survives_timeout_but_owned_immutable_id_is_removed(self):
        result, state = self.execute("present")
        self.assertTrue(state["accepted"])
        self.assertGreater(state["mutations"], 0)
        self.assertIsNone(state["active"])
        self.assertEqual(state["removed"], ["c" * 64])
        self.assertTrue(result["lanes"][3]["container"]["absenceObserved"])

    def test_late_registration_after_empty_scans_requires_disposal(self):
        result, state = self.execute("late", interrupt=True)
        self.assertTrue(state["accepted"])
        self.assertIsNone(state["active"])
        self.assertGreaterEqual(state["queries"], 3)
        self.assertTrue(result["lanes"][3]["container"]["absenceObserved"])
        self.rpc(op="release")
        state = self.rpc(op="state")
        self.assertIsNotNone(state["active"])
        self.assertGreater(state["mutations"], 0)
        self.assertEqual(state["removed"], [])
        self.assertTrue(result["requiresHostDisposal"])

    def test_wrong_owner_is_not_removed_and_primary_timeout_survives(self):
        result, state = self.execute("wrong-owner")
        self.assertIn("ownership mismatch", result["lanes"][3]["containerCleanupError"])
        self.assertEqual(state["removed"], [])
        self.assertIsNotNone(state["active"])

    def test_remove_failure_keeps_primary_timeout_and_disposal_requirement(self):
        result, state = self.execute("remove-failure")
        self.assertIn("removal failed: 19", result["lanes"][3]["containerCleanupError"])
        self.assertEqual(state["removed"], [])
        self.assertIsNotNone(state["active"])


if __name__ == "__main__":
    unittest.main()
