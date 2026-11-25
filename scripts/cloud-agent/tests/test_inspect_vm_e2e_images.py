import argparse
import hashlib
import contextlib
import importlib.util
import io
import json
import os
import shlex
import shutil
import signal
import sqlite3
import stat
import subprocess
import sys
import tarfile
import tempfile
import textwrap
import time
import unittest
import zipfile

from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "inspect_vm_e2e_images.py"
SPEC = importlib.util.spec_from_file_location("inspect_images", SCRIPT)
inspector = importlib.util.module_from_spec(SPEC)
with patch.object(sys, "path", [str(SCRIPT.parent), *sys.path]):
    SPEC.loader.exec_module(inspector)


class ImageInspectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.digest = hashlib.sha256(
            b"fixture critical payload, never executed"
        ).hexdigest()
        self.packages = {
            name: {
                "name": name,
                "nevra": f"{name}-0:0.31.0-1.azl3.x86_64",
                "sha256": hashlib.sha256(name.encode()).hexdigest(),
                "payload": {
                    filename: self.digest for filename in inspector.CRITICAL[name]
                },
            }
            for name in ("trident", "trident-service")
        }

    def archive(self, extra=None):
        archive = self.root / "artifact.zip"
        with zipfile.ZipFile(archive, "w") as target:
            target.writestr("go-tools/netlisten", b"fixture-tool-never-execute")
            target.writestr("private/id_rsa", b"fixture-only-do-not-extract")
            if extra:
                target.writestr(*extra)
        return archive

    def test_selected_zip_hash_and_no_private_member_extraction(self):
        target = self.root / "selected"
        files = inspector.extract_selected(self.archive(), target, ["netlisten"])
        self.assertEqual(list(target.iterdir()), [target / "netlisten"])
        self.assertEqual(files["netlisten"], inspector.measure(target / "netlisten"))

    def test_reviewed_whole_zip_guards(self):
        for unsafe in ("../escape", "/absolute", "drive:C", "a\\b"):
            with (
                self.subTest(unsafe=unsafe),
                self.assertRaises(inspector.ado.ArtifactError),
            ):
                inspector.extract_selected(
                    self.archive((unsafe, b"fixture")),
                    self.root / "selected",
                    ["netlisten"],
                )
        link = zipfile.ZipInfo("link")
        link.external_attr = (stat.S_IFLNK | 0o777) << 16
        with self.assertRaises(inspector.ado.ArtifactError):
            inspector.extract_selected(
                self.archive((link, b"target")), self.root / "selected", ["netlisten"]
            )

    def test_duplicate_basename_rejected(self):
        with self.assertRaises(inspector.ado.ArtifactError):
            inspector.extract_selected(
                self.archive(("other/netlisten", b"ambiguous")),
                self.root / "selected",
                ["netlisten"],
            )

    def test_rpm_nevra_and_ambiguous_installed_package(self):
        row = "trident\t0\t0.31.0\t1.azl3\tx86_64\n"
        self.assertEqual(
            inspector.rpm_rows(row), {"trident": self.packages["trident"]["nevra"]}
        )
        with self.assertRaisesRegex(inspector.Failure, "ambiguous-rpm"):
            inspector.rpm_rows(row + row)
        with self.assertRaisesRegex(inspector.Failure, "rpm-identity"):
            inspector.rpm_rows("trident\t0\thttps://fixture.invalid/?secret\t1\tx86_64")

    def test_source_rpm_payload_digests_are_sha256_regular_files(self):
        text = "".join(
            f"{path}\t{self.digest}\t{stat.S_IFREG | 0o755}\n"
            for path in inspector.CRITICAL["trident"]
        )

        def rpm_command(argv, **kwargs):
            if argv[0] == "rpm":
                return "8" if argv[-2] == "%{FILEDIGESTALGO}" else text
            kwargs["output"].write(b"fixture critical payload, never executed")
            return ""

        with patch.object(inspector, "command", side_effect=rpm_command):
            self.assertEqual(
                inspector.rpm_payload(self.root / "fixture.rpm", "trident"),
                self.packages["trident"]["payload"],
            )
        with patch.object(inspector, "command", return_value="1"):
            with self.assertRaisesRegex(inspector.Failure, "not-sha256"):
                inspector.rpm_payload(self.root / "fixture.rpm", "trident")
        with patch.object(inspector, "command", side_effect=["8", text, "", "", ""]):
            with self.assertRaisesRegex(inspector.Failure, "source-rpm-payload-digest"):
                inspector.rpm_payload(self.root / "fixture.rpm", "trident")

    def cosi(self, unsafe=None, declared_size=None):
        image = b"fixture raw filesystem, not mountable or bootable"
        metadata = {
            "version": "1.2",
            "osArch": "x86_64",
            "osPackages": [{"name": "trident-service", "version": "wrong-intent-only"}],
            "images": [
                {
                    "mountPoint": mount,
                    "fsType": "ext4",
                    "image": {
                        "path": f"images/{stem}.raw.zst",
                        "compressedSize": len(image),
                        "uncompressedSize": (
                            len(image) if declared_size is None else declared_size
                        ),
                        "sha384": hashlib.sha384(image).hexdigest(),
                    },
                }
                for mount, stem in (("/", "root"), ("/usr", "usr"))
            ],
        }
        path = self.root / "fixture.cosi"
        with tarfile.open(path, "w") as target:
            for name, body in (
                ("metadata.json", json.dumps(metadata).encode()),
                ("images/root.raw.zst", image),
                ("images/usr.raw.zst", image),
                *(([(unsafe, b"unsafe")]) if unsafe else []),
            ):
                entry = tarfile.TarInfo(name)
                entry.size = len(body)
                target.addfile(entry, io.BytesIO(body))
        return path

    def test_cosi_separate_usr_and_member_hashes(self):
        def decompress(compressed, raw, expected_size):
            self.assertEqual(expected_size, compressed.stat().st_size)
            raw.write_bytes(compressed.read_bytes())

        with patch.object(inspector, "decompress_cosi", side_effect=decompress):
            drives, mounts = inspector.cosi_layout(self.cosi(), self.root)
        self.assertEqual(mounts, [("/dev/sda", "/"), ("/dev/sdb", "/usr")])
        self.assertEqual(len(drives), 2)

    def test_cosi_traversal_rejected_without_extraction(self):
        with self.assertRaisesRegex(inspector.Failure, "unsafe-member"):
            inspector.cosi_layout(self.cosi("../escape"), self.root)
        self.assertFalse((self.root.parent / "escape").exists())

    def test_workflow_admits_only_exact_owned_branch_push(self):
        workflow = SCRIPT.parents[2] / ".github/workflows/vm-e2e-image-inspection.yml"
        self.assertIn(
            "on:\n  push:\n    branches:\n"
            "      - user/britel/vm-e2e-image-inspection-20261123\n",
            workflow.read_text(),
        )

    def test_guestfish_readiness_skips_apt_when_present(self):
        with (
            patch.object(inspector.shutil, "which", return_value="/usr/bin/guestfish"),
            patch.object(inspector, "command") as run,
        ):
            inspector.ensure_guestfish()
        run.assert_called_once_with(
            ["guestfish", "--version"],
            timeout=30,
            failure_fact="guestfish-readiness-failed",
        )

    def test_guestfish_installs_only_missing_minimal_package_with_bounds(self):
        with (
            patch.object(
                inspector.shutil, "which", side_effect=[None, "/usr/bin/guestfish"]
            ),
            patch.object(inspector.os, "geteuid", return_value=0),
            patch.object(inspector, "command") as run,
        ):
            inspector.ensure_guestfish()
        self.assertEqual(
            run.call_args_list[0].args[0],
            ["apt-get", "-o", "DPkg::Lock::Timeout=60", "update"],
        )
        self.assertEqual(
            run.call_args_list[1].args[0],
            [
                "env",
                "DEBIAN_FRONTEND=noninteractive",
                "NEEDRESTART_MODE=l",
                "apt-get",
                "-o",
                "DPkg::Lock::Timeout=60",
                "install",
                "-y",
                "--no-install-recommends",
                "guestfish",
            ],
        )
        self.assertEqual(
            [call.kwargs["timeout"] for call in run.call_args_list], [180, 300, 30]
        )

    def publication_args(self, name="publication"):
        output = self.root / name / "inspection.json"
        inspector.prepare_publication(output)
        return argparse.Namespace(
            output=output,
            scratch=self.root,
            publication_uid=os.getuid(),
            publication_gid=os.getgid(),
        )

    def test_privileged_context_keeps_profile_private_and_handoff_exact(self):
        args = self.publication_args()
        touched = []

        def inspect(output, scratch, bootstrap_guestfish):
            self.assertTrue(bootstrap_guestfish)
            self.assertEqual(os.environ["LIBGUESTFS_BACKEND"], "direct")
            self.assertNotEqual(os.environ["AZURE_CONFIG_DIR"], "caller-cache")
            self.assertEqual(
                Path(os.environ["AZURE_CONFIG_DIR"]).parent, Path(os.environ["HOME"])
            )
            self.assertEqual(
                os.environ["AZURE_EXTENSION_DIR"], "fixture-installed-extension-code"
            )
            self.assertEqual(os.environ["GIT_CONFIG_KEY_0"], "safe.directory")
            self.assertEqual(os.environ["GIT_CONFIG_VALUE_0"], str(SCRIPT.parents[2]))
            inspector.failed_publication(output, "fixture-primary-inspection-failure")
            return 7

        def handoff(fd, uid, gid):
            touched.append(Path(os.readlink(f"/proc/self/fd/{fd}")))
            self.assertEqual((uid, gid), (os.getuid(), os.getgid()))

        with (
            patch.dict(
                os.environ,
                {
                    "AZURE_CONFIG_DIR": "caller-cache",
                    "AZURE_EXTENSION_DIR": "fixture-installed-extension-code",
                },
            ),
            patch.object(inspector.os, "geteuid", return_value=0),
            patch.object(inspector, "execute", side_effect=inspect),
            patch.object(inspector.os, "fchown", side_effect=handoff),
        ):
            self.assertEqual(inspector.privileged_inspection(args), 7)
        self.assertEqual(touched, [args.output, args.output.parent])
        self.assertEqual(stat.S_IMODE(args.output.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(args.output.parent.stat().st_mode), 0o700)
        self.assertEqual(
            json.loads(args.output.read_text())["failure"]["missingFact"],
            "fixture-primary-inspection-failure",
        )
        self.assertEqual(list(self.root.iterdir()), [args.output.parent])

    def test_handoff_failure_preserves_primary_exit_and_redacts_error(self):
        args = self.publication_args()
        stderr = io.StringIO()
        for primary, expected in ((7, 7), (130, 130), (0, 1)):
            with (
                patch.dict(os.environ),
                patch.object(inspector.os, "geteuid", return_value=0),
                patch.object(inspector, "execute", return_value=primary),
                patch.object(
                    inspector.os,
                    "fchown",
                    side_effect=PermissionError("fixture-sensitive"),
                ),
                contextlib.redirect_stderr(stderr),
            ):
                self.assertEqual(inspector.privileged_inspection(args), expected)
        self.assertNotIn("fixture-sensitive", stderr.getvalue())
        self.assertIn("Exact report publication failed", stderr.getvalue())

    def test_publication_rejects_symlink_before_inspection_or_ownership_change(self):
        args = self.publication_args()
        other = self.root / "unrelated-fixture"
        other.write_bytes(b"fixture-only-do-not-touch")
        args.output.unlink()
        args.output.symlink_to(other)
        with (
            patch.object(inspector.os, "geteuid", return_value=0),
            patch.object(inspector, "execute") as run,
            patch.object(inspector.os, "fchown") as chown,
            self.assertRaises(OSError),
        ):
            inspector.privileged_inspection(args)
        run.assert_not_called()
        chown.assert_not_called()
        self.assertEqual(other.read_bytes(), b"fixture-only-do-not-touch")

    def publication_substitution_fixture(self, kind):
        for phase in (
            "complete",
            "failure",
            "cancelled",
            "setup-failure",
            "setup-cancelled",
        ):
            with self.subTest(kind=kind, phase=phase):
                args = self.publication_args(f"{kind}-{phase}")
                unrelated = self.root / f"{kind}-{phase}-unrelated"
                unrelated.mkdir(mode=0o700)
                sentinel = unrelated / "inspection.json"
                sentinel.write_bytes(b"fixture-unrelated-private-sentinel")
                sentinel.chmod(0o600)
                before = sentinel.stat()
                retained = (
                    args.output.with_name("retained.json")
                    if kind == "file"
                    else args.output.parent.with_name(
                        args.output.parent.name + "-retained"
                    )
                    / args.output.name
                )

                def substitute(*unused, **unused_kwargs):
                    if kind == "file":
                        args.output.rename(retained)
                        args.output.symlink_to(sentinel)
                    else:
                        args.output.parent.rename(retained.parent)
                        args.output.parent.symlink_to(
                            unrelated, target_is_directory=True
                        )
                    if phase.endswith("cancelled"):
                        raise inspector.Cancelled()
                    if phase == "setup-failure":
                        raise RuntimeError("fixture-sensitive-setup-error")
                    if phase == "failure":
                        raise inspector.Failure("fixture-primary-inspection-failure")

                stderr = io.StringIO()
                with contextlib.ExitStack() as stack:
                    stack.enter_context(
                        patch.dict(os.environ, {"EXCLUSIVE_DISPOSABLE_1ES": "true"})
                    )
                    stack.enter_context(
                        patch.object(inspector.os, "geteuid", return_value=0)
                    )
                    chown = stack.enter_context(patch.object(inspector.os, "fchown"))
                    stack.enter_context(contextlib.redirect_stderr(stderr))
                    if phase.startswith("setup-"):
                        stack.enter_context(
                            patch.object(inspector, "execute", side_effect=substitute)
                        )
                    else:
                        stack.enter_context(patch.object(inspector, "ensure_guestfish"))
                        stack.enter_context(
                            patch.object(
                                inspector.shutil, "which", return_value=str(SCRIPT)
                            )
                        )
                        stack.enter_context(
                            patch.object(
                                inspector,
                                "command",
                                side_effect=lambda argv, **kw: (
                                    "a" * 40 if argv[0] == "git" else str(self.root)
                                ),
                            )
                        )
                        stack.enter_context(
                            patch.object(inspector, "inspect", side_effect=substitute)
                        )
                    code = inspector.privileged_inspection(args)
                self.assertEqual(
                    sentinel.read_bytes(), b"fixture-unrelated-private-sentinel"
                )
                after = sentinel.stat()
                self.assertEqual(
                    (after.st_ino, after.st_mode, after.st_uid, after.st_gid),
                    (before.st_ino, before.st_mode, before.st_uid, before.st_gid),
                )
                chown.assert_not_called()
                self.assertEqual(code, 130 if phase.endswith("cancelled") else 1)
                self.assertIn("Exact report publication failed", stderr.getvalue())
                self.assertNotIn("fixture-sensitive", stderr.getvalue())
                report = json.loads(retained.read_text())
                self.assertEqual(report["status"], "failure")
                self.assertEqual(report["reviewStatus"], "UNREVIEWED")
                if phase == "complete":
                    self.assertEqual(
                        report["failure"]["missingFact"],
                        "exact-report-publication-failed",
                    )
                elif phase == "failure":
                    self.assertEqual(
                        report["failure"]["missingFact"],
                        "fixture-primary-inspection-failure",
                    )
                elif phase == "cancelled":
                    self.assertEqual(report["failure"], {"kind": "cancelled"})
                else:
                    expected = (
                        "privileged-inspection-cancelled"
                        if phase.endswith("cancelled")
                        else "privileged-inspection-setup-failed"
                    )
                    self.assertEqual(report["failure"]["missingFact"], expected)

    def test_pinned_report_rejects_file_replacement_for_all_writers(self):
        self.publication_substitution_fixture("file")

    def test_pinned_report_rejects_parent_substitution_for_all_writers(self):
        self.publication_substitution_fixture("parent")

    def test_bootstrap_failures_use_unchanged_unreviewed_report_before_login(self):
        for fact in (
            "guestfish-package-index-failed",
            "guestfish-package-install-failed",
            "guestfish-readiness-failed",
            "guestfish-missing-after-bootstrap",
        ):
            with self.subTest(fact=fact):
                args = self.publication_args(fact)

                def fail_stage(argv, **kwargs):
                    self.assertNotIn("az", argv)
                    if kwargs["failure_fact"] == fact:
                        raise inspector.Failure(fact)

                found = None if fact.endswith("after-bootstrap") else "/bin/guestfish"
                with (
                    patch.dict(os.environ, {"EXCLUSIVE_DISPOSABLE_1ES": "true"}),
                    patch.object(inspector.os, "geteuid", return_value=0),
                    patch.object(inspector.os, "fchown"),
                    patch.object(inspector.shutil, "which", side_effect=[None, found]),
                    patch.object(inspector, "command", side_effect=fail_stage),
                ):
                    self.assertEqual(inspector.privileged_inspection(args), 1)
                report = json.loads(args.output.read_text())
                self.assertEqual(set(report), set(inspector.new_report()))
                self.assertEqual(report["status"], "failure")
                self.assertEqual(report["reviewStatus"], "UNREVIEWED")
                self.assertEqual(report["complete"], {})
                self.assertEqual(report["failure"]["missingFact"], fact)

    def test_privileged_setup_cancellation_and_owner_validation_fail_closed(self):
        args = self.publication_args()
        with (
            patch.dict(os.environ),
            patch.object(inspector.os, "geteuid", return_value=0),
            patch.object(inspector.os, "fchown"),
            patch.object(inspector, "execute", side_effect=inspector.Cancelled()),
        ):
            self.assertEqual(inspector.privileged_inspection(args), 130)
        report = json.loads(args.output.read_text())
        self.assertEqual(
            report["failure"]["missingFact"], "privileged-inspection-cancelled"
        )
        args.publication_uid += 1
        with (
            patch.object(inspector.os, "geteuid", return_value=0),
            patch.object(inspector.os, "fchown") as chown,
            patch.object(inspector, "execute") as run,
            self.assertRaisesRegex(
                inspector.Failure, "publication-not-private-and-job-owned"
            ),
        ):
            inspector.privileged_inspection(args)
        run.assert_not_called()
        chown.assert_not_called()

    def test_missing_guestfish_never_installs_without_root(self):
        with (
            patch.object(inspector.shutil, "which", return_value=None),
            patch.object(inspector.os, "geteuid", return_value=1000),
            patch.object(inspector, "command") as run,
            self.assertRaisesRegex(
                inspector.Failure, "guestfish-bootstrap-requires-root"
            ),
        ):
            inspector.ensure_guestfish()
        run.assert_not_called()

    def test_workflow_sudo_failure_and_primary_exit_remain_readable_and_redacted(self):
        workflow = SCRIPT.parents[2] / ".github/workflows/vm-e2e-image-inspection.yml"
        body = textwrap.dedent(
            workflow.read_text()
            .split("        run: |\n", 1)[1]
            .split("\n      - name:", 1)[0]
        )
        self.assertIn("--prepare-publication", body)
        self.assertIn("sudo -n env", body)
        self.assertLess(body.index("sudo -n env"), body.index("timeout --signal"))
        stub = """
sudo() {
  "$TEST_PYTHON" -c 'import json,os,sys; from pathlib import Path; Path(os.environ["TEST_ARGS"]).write_text(json.dumps(sys.argv[1:]))' "$@"
  printf 'fixture-sensitive-output\\n' >&2
  return "$TEST_EXIT"
}
id() {
  if [[ "$1" == "-u" ]]; then printf '12345\\n'; else printf '23456\\n'; fi
}
"""
        for exit_code in (1, 7):
            with self.subTest(exit_code=exit_code):
                scratch = self.root / str(exit_code)
                scratch.mkdir()
                captured = scratch / "sudo-args.json"
                result = subprocess.run(
                    ["bash", "-e", "-o", "pipefail", "-c", stub + body],
                    cwd=SCRIPT.parents[2],
                    env={
                        **os.environ,
                        "RUNNER_TEMP": str(scratch),
                        "TEST_PYTHON": sys.executable,
                        "TEST_ARGS": str(captured),
                        "TEST_EXIT": str(exit_code),
                        "AZURE_EXTENSION_DIR": "fixture-installed-extension-code",
                    },
                    capture_output=True,
                    timeout=5,
                )
                self.assertEqual(result.returncode, exit_code)
                self.assertNotIn(b"fixture-sensitive", result.stdout + result.stderr)
                argv = json.loads(captured.read_text())
                self.assertEqual(argv[:2], ["-n", "env"])
                self.assertIn(
                    "AZURE_EXTENSION_DIR=fixture-installed-extension-code", argv
                )
                self.assertIn("EXCLUSIVE_DISPOSABLE_1ES=true", argv)
                self.assertEqual(argv[argv.index("--publication-uid") + 1], "12345")
                self.assertEqual(argv[argv.index("--publication-gid") + 1], "23456")
                self.assertIn("--bootstrap-guestfish", argv)
                report_path = (
                    scratch / "trident-image-inspection-report/inspection.json"
                )
                report = json.loads(report_path.read_text())
                self.assertEqual(
                    report["failure"]["missingFact"],
                    "privileged-inspection-not-started",
                )
                self.assertEqual(stat.S_IMODE(report_path.stat().st_mode), 0o600)
                self.assertEqual(stat.S_IMODE(report_path.parent.stat().st_mode), 0o700)

    @contextlib.contextmanager
    def leader_first_fixture(self, *, overflow=False, cancel=False):
        import ctypes

        libc = ctypes.CDLL(None, use_errno=True)
        previous = ctypes.c_int()
        self.assertEqual(libc.prctl(37, ctypes.byref(previous), 0, 0, 0), 0)
        self.assertEqual(libc.prctl(36, 1, 0, 0, 0), 0)
        pidfile = self.root / "descendant.pid"
        exited = self.root / "leader-exited"
        state = {"processes": [], "reaped": False}
        original = subprocess.Popen
        fixture = """
import os
from pathlib import Path
import signal
import sys
import time
pidfile, exited = map(Path, sys.argv[1:3])
def exit_leader(signum, frame):
    exited.write_text("graceful exit")
    sys.exit(0)
signal.signal(signal.SIGTERM, exit_leader)
if os.fork() == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    pidfile.write_text(str(os.getpid()))
else:
    deadline = time.monotonic() + 2
    while not pidfile.exists():
        if time.monotonic() >= deadline:
            sys.exit(2)
        time.sleep(0.01)
    if sys.argv[3] == "overflow":
        sys.stdout.buffer.write(b"x" * (2 * 1024 * 1024))
        sys.stdout.buffer.flush()
while True:
    time.sleep(0.01)
"""

        def capture(argv, **kwargs):
            process = original(
                [
                    sys.executable,
                    "-c",
                    fixture,
                    str(pidfile),
                    str(exited),
                    "overflow" if overflow else "idle",
                ],
                **kwargs,
            )
            state["processes"].append(process)
            communicate = process.communicate

            def bounded_communicate(data, timeout):
                if cancel:
                    deadline = time.monotonic() + 2
                    while not pidfile.exists() and time.monotonic() < deadline:
                        time.sleep(0.01)
                    self.assertTrue(pidfile.exists(), "Fixture did not become ready")
                    raise inspector.Cancelled()
                return communicate(data, min(timeout, 2))

            process.communicate = bounded_communicate
            return process

        try:
            with patch.object(inspector.subprocess, "Popen", side_effect=capture):
                yield state, pidfile, exited
        finally:
            # Always kill/reap only this fixture's groups, even if assertions fail.
            try:
                for process in state["processes"]:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        process.poll()
                    process.wait(timeout=2)
                    deadline = time.monotonic() + 2
                    while True:
                        try:
                            child, _ = os.waitpid(-process.pid, os.WNOHANG)
                        except ChildProcessError:
                            break
                        if not child:
                            self.assertLess(
                                time.monotonic(), deadline, "Fixture cleanup timed out"
                            )
                            time.sleep(0.01)
            finally:
                self.assertEqual(libc.prctl(36, previous.value, 0, 0, 0), 0)

    def assert_fixture_group_stopped(self, state, pidfile, exited):
        self.assertTrue(exited.exists(), "Leader did not exit gracefully")
        self.assertIsNotNone(state["processes"][0].poll())
        self.assertTrue(pidfile.exists(), "Fixture descendant never became ready")
        child_pid = int(pidfile.read_text())
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            pid, status = os.waitpid(child_pid, os.WNOHANG)
            if pid:
                state["reaped"] = True
                self.assertTrue(os.WIFSIGNALED(status))
                self.assertEqual(os.WTERMSIG(status), signal.SIGKILL)
                return
            time.sleep(0.01)
        self.fail("Owned descendant survived leader-first teardown")

    @unittest.skipUnless(sys.platform == "linux", "requires Linux child subreaping")
    def test_timeout_kills_descendant_after_leader_exits(self):
        with self.leader_first_fixture() as (state, pidfile, exited):
            with self.assertRaises(subprocess.TimeoutExpired):
                inspector.command(["fixture"], timeout=1)
            self.assert_fixture_group_stopped(state, pidfile, exited)

    @unittest.skipUnless(sys.platform == "linux", "requires Linux child subreaping")
    def test_cancellation_kills_descendant_and_preserves_cancellation(self):
        with self.leader_first_fixture(cancel=True) as (state, pidfile, exited):
            with self.assertRaises(inspector.Cancelled):
                inspector.command(["fixture"], timeout=1)
            self.assert_fixture_group_stopped(state, pidfile, exited)

    @unittest.skipUnless(sys.platform == "linux", "requires Linux child subreaping")
    def test_cosi_overflow_never_writes_beyond_declared_size_and_reaps_group(self):
        path = self.cosi(declared_size=1)
        raw = self.root / "part-0.raw"
        written = []
        writes = []
        unlink = Path.unlink
        open_file = Path.open

        def record_writes(path, *args, **kwargs):
            stream = open_file(path, *args, **kwargs)
            if path == raw and args[0] == "xb":
                write = stream.write

                def record(data):
                    writes.append(len(data))
                    return write(data)

                stream.write = record
            return stream

        def record_cleanup(path, *args, **kwargs):
            if path == raw and path.exists():
                written.append(path.stat().st_size)
            return unlink(path, *args, **kwargs)

        with self.leader_first_fixture(overflow=True) as (state, pidfile, exited):
            with (
                patch.object(Path, "unlink", record_cleanup),
                patch.object(Path, "open", record_writes),
            ):
                with self.assertRaisesRegex(
                    inspector.Failure, "cosi-raw-size-overflow"
                ):
                    inspector.cosi_layout(path, self.root)
            self.assertTrue(written, "Incomplete owned raw file was not cleaned")
            self.assertLessEqual(max(written), 1)
            self.assertLessEqual(sum(writes), 1)
            self.assertFalse(raw.exists())
            self.assert_fixture_group_stopped(state, pidfile, exited)

    def test_synthetic_zstd_exact_and_truncated_decompression(self):
        payload = b"synthetic fixture bytes, never booted\n" * 100
        compressed = self.root / "fixture.zst"
        result = subprocess.run(
            ["zstd", "-c"],
            input=payload,
            capture_output=True,
            check=True,
            timeout=5,
        )
        compressed.write_bytes(result.stdout)
        raw = self.root / "exact.raw"
        inspector.decompress_cosi(compressed, raw, len(payload))
        self.assertEqual(raw.read_bytes(), payload)
        truncated = self.root / "truncated.raw"
        with self.assertRaisesRegex(inspector.Failure, "^cosi-raw-size$"):
            inspector.decompress_cosi(compressed, truncated, len(payload) + 1)
        self.assertFalse(truncated.exists())

    def test_decompression_absolute_ceiling_rejects_before_spawn(self):
        with (
            patch.object(inspector.subprocess, "Popen") as popen,
            self.assertRaisesRegex(inspector.Failure, "cosi-image-layout"),
        ):
            inspector.decompress_cosi(
                self.root / "fixture.zst",
                self.root / "oversize.raw",
                16 * 1024**3 + 1,
            )
        popen.assert_not_called()
        self.assertFalse((self.root / "oversize.raw").exists())

    @unittest.skipUnless(sys.platform == "linux", "requires Linux child subreaping")
    def test_decompression_timeout_removes_raw_and_reaps_whole_group(self):
        raw = self.root / "timeout.raw"
        with self.leader_first_fixture() as (state, pidfile, exited):
            with self.assertRaises(subprocess.TimeoutExpired):
                inspector.decompress_cosi(self.root / "fixture.zst", raw, 1, timeout=1)
            self.assertFalse(raw.exists())
            self.assert_fixture_group_stopped(state, pidfile, exited)

    def installed_rows(self):
        return "".join(f"{name}\t0\t0.31.0\t1.azl3\tx86_64\n" for name in self.packages)

    def inspect_fixture(self, guest_answers, installed=None, suffix=".cosi"):
        work = Path(tempfile.mkdtemp(dir=self.root, prefix="fixture-"))
        if guest_answers[0] == "true\ntrue\n":
            # The usual AZL RPMDB aliases must resolve to identical database bytes.
            guest_answers = [
                guest_answers[0],
                (self.digest + "\n") * 2,
                *guest_answers[1:],
            ]
        with (
            patch.object(
                inspector,
                "cosi_layout",
                return_value=(
                    [(self.root / "root.raw", "raw")],
                    [("/dev/sda", "/")],
                ),
            ),
            patch.object(inspector, "guest", side_effect=guest_answers),
            patch.object(inspector, "checkpoint_rpmdb"),
            patch.object(
                inspector,
                "command",
                return_value=(
                    self.installed_rows() if installed is None else installed
                ),
            ),
        ):
            return inspector.inspect_image(
                self.root / ("fixture" + suffix), work, self.packages
            )

    def test_actual_rpmdb_and_each_critical_payload_required(self):
        count = sum(len(row["payload"]) for row in self.packages.values())
        proof = self.inspect_fixture(
            ["true\ntrue\n", "false\n" * 4, "", (self.digest + "\n") * count]
        )
        self.assertEqual(len(proof["installedPackages"]), 2)
        self.assertEqual(len(proof["payloadSha256"]), count)
        self.assertEqual(proof["basis"], "rpmdb-and-read-only-payload")

    def test_rpmdb_name_only_or_nevra_mismatch_cannot_pass(self):
        for installed in (
            "trident\t0\t0.31.0\t1.azl3\tx86_64\n",
            self.installed_rows().replace("0.31.0", "0.30.0"),
        ):
            with (
                self.subTest(installed=installed),
                self.assertRaises(inspector.Failure),
            ):
                self.inspect_fixture(["true\ntrue\n", "false\n" * 4, ""], installed)

    def test_missing_rpmdb_or_unsupported_sidecars_fail(self):
        for answers in (
            ["false\nfalse\n"],
            ["true\nunexpected\n"],
            ["true\ntrue\n", "true\n"],
            ["true\ntrue\n", "false\ntrue\n" * 2],
            ["true\ntrue\n", "true\nfalse\nfalse\nfalse\n"],
            ["true\nfalse\n", "true\nfalse\n", "false\n"],
        ):
            with self.subTest(answers=answers), self.assertRaises(inspector.Failure):
                self.inspect_fixture(answers)

    def sqlite_snapshot(self):
        source = Path(tempfile.mkdtemp(dir=self.root, prefix="sqlite-source-"))
        connection = sqlite3.connect(source / "rpmdb.sqlite")
        self.addCleanup(connection.close)
        connection.execute("CREATE TABLE fixture(value TEXT)")
        connection.execute("INSERT INTO fixture VALUES ('main-only')")
        connection.commit()
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute("PRAGMA wal_autocheckpoint=0")
        connection.execute("UPDATE fixture SET value='committed-in-wal'")
        connection.commit()
        return source, connection

    def copy_sqlite_snapshot(self, source, wal=True):
        target = Path(tempfile.mkdtemp(dir=self.root, prefix="sqlite-copy-"))
        for suffix in ("", "-wal") if wal else ("",):
            shutil.copyfile(
                source / ("rpmdb.sqlite" + suffix), target / ("rpmdb.sqlite" + suffix)
            )
        return target

    def test_checkpoint_includes_committed_wal_without_changing_source(self):
        source, connection = self.sqlite_snapshot()
        # An uncommitted transaction is not installed-package state.
        wal = source / "rpmdb.sqlite-wal"
        committed_size = wal.stat().st_size
        connection.execute("PRAGMA cache_size=1")
        connection.execute("UPDATE fixture SET value=?", ("uncommitted" * 10000,))
        self.assertGreater(wal.stat().st_size, committed_size)
        before = {path.name: path.read_bytes() for path in source.iterdir()}
        target = self.copy_sqlite_snapshot(source)
        self.assertFalse((target / "rpmdb.sqlite-shm").exists())
        inspector.checkpoint_rpmdb(target)
        with contextlib.closing(sqlite3.connect(target / "rpmdb.sqlite")) as db:
            self.assertEqual(
                db.execute("SELECT value FROM fixture").fetchall(),
                [("committed-in-wal",)],
            )
        self.assertEqual(
            {path.name: path.read_bytes() for path in source.iterdir()}, before
        )

    def test_main_only_and_empty_wal_are_supported(self):
        source, _ = self.sqlite_snapshot()
        for empty_wal in (False, True):
            with self.subTest(empty_wal=empty_wal):
                target = self.copy_sqlite_snapshot(source, wal=False)
                if empty_wal:
                    (target / "rpmdb.sqlite-wal").touch()
                inspector.checkpoint_rpmdb(target)
                with contextlib.closing(sqlite3.connect(target / "rpmdb.sqlite")) as db:
                    self.assertEqual(
                        db.execute("SELECT value FROM fixture").fetchall(),
                        [("main-only",)],
                    )

    def test_nonempty_wal_without_recoverable_commit_fails(self):
        source, _ = self.sqlite_snapshot()
        wal = (source / "rpmdb.sqlite-wal").read_bytes()
        for content in (b"invalid-wal", wal[:32], bytes(len(wal))):
            with self.subTest(length=len(content)):
                target = self.copy_sqlite_snapshot(source)
                (target / "rpmdb.sqlite-wal").write_bytes(content)
                with self.assertRaisesRegex(
                    inspector.Failure, "image-rpmdb-wal-unrecoverable"
                ):
                    inspector.checkpoint_rpmdb(target)

    def test_invalid_sqlite_and_integrity_failure_are_sanitized(self):
        target = self.root / "invalid-db"
        target.mkdir()
        db = target / "rpmdb.sqlite"
        db.write_bytes(b"fixture-private-corrupt-database")
        with self.assertRaisesRegex(inspector.Failure, "^image-rpmdb-sqlite-invalid$"):
            inspector.checkpoint_rpmdb(target)
        db.unlink()
        with contextlib.closing(sqlite3.connect(db)) as connection:
            connection.execute("CREATE TABLE fixture(value TEXT)")
            connection.execute("CREATE TABLE other(value TEXT)")
            connection.execute("PRAGMA writable_schema=ON")
            connection.execute(
                "UPDATE sqlite_master SET rootpage="
                "(SELECT rootpage FROM sqlite_master WHERE name='other') "
                "WHERE name='fixture'"
            )
            connection.commit()
        with self.assertRaisesRegex(inspector.Failure, "^image-rpmdb-integrity$"):
            inspector.checkpoint_rpmdb(target)

    def test_checkpoint_busy_and_cancellation_remain_failures(self):
        source, _ = self.sqlite_snapshot()
        target = self.copy_sqlite_snapshot(source)
        with contextlib.closing(sqlite3.connect(target / "rpmdb.sqlite")) as reader:
            reader.execute("BEGIN")
            reader.execute("SELECT * FROM fixture").fetchall()
            with contextlib.closing(sqlite3.connect(target / "rpmdb.sqlite")) as writer:
                writer.execute("UPDATE fixture SET value='newer-commit'")
                writer.commit()
            with self.assertRaisesRegex(
                inspector.Failure, "^image-rpmdb-wal-unrecoverable$"
            ):
                inspector.checkpoint_rpmdb(target)
        primary = inspector.Cancelled()
        with patch.object(inspector.sqlite3, "connect") as connect:
            connect.return_value.execute.side_effect = primary
            with self.assertRaises(inspector.Cancelled) as caught:
                inspector.checkpoint_rpmdb(target)
            self.assertIs(caught.exception, primary)
            connect.return_value.close.assert_called_once_with()

    def test_image_wal_recovery_still_requires_nevra_and_payload_validation(self):
        source, _ = self.sqlite_snapshot()
        before = {path.name: path.read_bytes() for path in source.iterdir()}
        for suffix, wrong in (
            (".iso", None),
            (".cosi", None),
            (".cosi", "nevra"),
            (".cosi", "payload"),
            (".cosi", "wal"),
            (".cosi", "missing-wal"),
        ):
            work = Path(tempfile.mkdtemp(dir=self.root, prefix="image-wal-"))
            commands = []

            def guest(drives, mounts, script):
                commands.extend(script)
                if script == ["is-file /liveos/rootfs.img"]:
                    return "true\n"
                if (
                    script[0].startswith("is-file ")
                    and "rpmdb.sqlite-wal" not in script[0]
                ):
                    return "true\nfalse\n"
                if script[0].startswith("exists "):
                    return "true\nfalse\n"
                if script[0].startswith("is-file "):
                    return "true\n"
                if script[0].startswith("download "):
                    for line in script:
                        _, remote, local = shlex.split(line)
                        if remote == "/liveos/rootfs.img":
                            Path(local).write_bytes(b"fixture image, never mounted")
                        elif remote.endswith("-wal") and wrong == "missing-wal":
                            continue
                        elif remote.endswith("-wal") and wrong == "wal":
                            Path(local).write_bytes(b"fixture-invalid-wal")
                        else:
                            shutil.copyfile(source / Path(remote).name, local)
                    return ""
                self.assertTrue(
                    all(line.startswith("checksum sha256 ") for line in script)
                )
                return ((self.digest if wrong != "payload" else "0" * 64) + "\n") * len(
                    script
                )

            def rpm(argv, **kwargs):
                self.assertEqual(argv[:3], ["rpm", "--noplugins", "--dbpath"])
                with contextlib.closing(
                    sqlite3.connect(Path(argv[3]) / "rpmdb.sqlite")
                ) as db:
                    self.assertEqual(
                        db.execute("SELECT value FROM fixture").fetchall(),
                        [("committed-in-wal",)],
                    )
                rows = self.installed_rows()
                return rows.replace("0.31.0", "0.30.0") if wrong == "nevra" else rows

            with (
                self.subTest(suffix=suffix, wrong=wrong),
                patch.object(inspector, "cosi_layout", return_value=([], [])),
                patch.object(inspector, "guest", side_effect=guest),
                patch.object(inspector, "command", side_effect=rpm) as rpm_command,
            ):
                if wrong:
                    with self.assertRaisesRegex(
                        inspector.Failure,
                        {
                            "nevra": "image-rpm-nevra-mismatch",
                            "payload": "image-critical-payload-mismatch",
                            "wal": "image-rpmdb-wal-unrecoverable",
                            "missing-wal": "image-rpmdb-wal-missing",
                        }[wrong],
                    ):
                        inspector.inspect_image(
                            self.root / ("fixture" + suffix), work, self.packages
                        )
                    if wrong in ("wal", "missing-wal"):
                        rpm_command.assert_not_called()
                else:
                    proof = inspector.inspect_image(
                        self.root / ("fixture" + suffix), work, self.packages
                    )
                    self.assertEqual(len(proof["installedPackages"]), 2)
                    self.assertEqual(proof["basis"], "rpmdb-and-read-only-payload")
            self.assertTrue(any("rpmdb.sqlite-wal " in line for line in commands))
            self.assertFalse(any("-shm" in line for line in commands))
        self.assertEqual(
            {path.name: path.read_bytes() for path in source.iterdir()}, before
        )

    def test_equal_main_databases_with_distinct_wals_fail_closed(self):
        with self.assertRaisesRegex(inspector.Failure, "image-ambiguous-rpmdb"):
            self.inspect_fixture(
                [
                    "true\ntrue\n",
                    "true\nfalse\n" * 2,
                    "true\ntrue\n",
                    self.digest + "\n" + "0" * 64 + "\n",
                ]
            )

    def test_distinct_rpmdb_locations_fail_closed(self):
        with (
            patch.object(inspector, "cosi_layout", return_value=([], [])),
            patch.object(
                inspector,
                "guest",
                side_effect=["true\ntrue\n", self.digest + "\n" + "0" * 64 + "\n"],
            ),
            self.assertRaisesRegex(inspector.Failure, "image-ambiguous-rpmdb"),
        ):
            inspector.inspect_image(
                self.root / "fixture.cosi", self.root, self.packages
            )

    def test_payload_mismatch_and_inspection_command_failure_fail(self):
        with self.assertRaisesRegex(inspector.Failure, "payload-mismatch"):
            self.inspect_fixture(
                ["true\ntrue\n", "false\n" * 4, "", ("0" * 64 + "\n") * 4]
            )
        with patch.object(
            inspector,
            "guest",
            side_effect=inspector.Failure("inspection-command-failed"),
        ):
            with self.assertRaisesRegex(inspector.Failure, "command-failed"):
                inspector.inspect_image(
                    self.root / "fixture.iso", self.root, self.packages
                )

    def test_iso_layout_missing_is_not_package_intent_success(self):
        with self.assertRaisesRegex(inspector.Failure, "iso-missing-liveos-rootfs"):
            self.inspect_fixture(["false\n"], suffix=".iso")

    def test_guestfish_readonly_no_network_or_payload_execution(self):
        with patch.object(inspector, "command", return_value="true\n") as run:
            inspector.guest(
                [(self.root / "root.raw", "raw")],
                [("/dev/sda", "/")],
                ["is-file /var/lib/rpm/rpmdb.sqlite"],
            )
        argv = run.call_args.args[0]
        self.assertEqual(argv[:2], ["guestfish", "--ro"])
        self.assertNotIn("--network", argv)
        self.assertEqual(
            run.call_args.kwargs["data"],
            "run\nmount-ro /dev/sda /\nis-file /var/lib/rpm/rpmdb.sqlite\n",
        )

    def execute_fixture(self, error):
        output = self.root / "report.json"
        with (
            patch.dict(os.environ, {"EXCLUSIVE_DISPOSABLE_1ES": "true"}),
            patch.object(inspector.shutil, "which", return_value=str(SCRIPT)),
            patch.object(
                inspector,
                "command",
                side_effect=lambda argv, **kw: (
                    "a" * 40 if argv[0] == "git" else str(self.root)
                ),
            ),
            patch.object(inspector, "inspect", side_effect=error),
        ):
            code = inspector.execute(output, self.root)
        return code, output.read_text(), json.loads(output.read_text())

    def test_failure_report_never_serializes_helper_error_body_or_urls(self):
        code, raw, report = self.execute_fixture(
            inspector.ado.ArtifactError(
                "https://fixture.invalid/?sig=fixture-sensitive-body"
            )
        )
        self.assertEqual(code, 1)
        self.assertEqual(report["status"], "failure")
        self.assertEqual(report["reviewStatus"], "UNREVIEWED")
        self.assertNotIn("fixture-sensitive", raw)
        self.assertNotIn("fixture.invalid", raw)
        self.assertNotIn("review", report)

    def test_cancelled_report_and_owned_scratch_cleanup(self):
        code, _, report = self.execute_fixture(inspector.Cancelled())
        self.assertEqual(code, 130)
        self.assertEqual(report["failure"], {"kind": "cancelled"})
        self.assertEqual(list(self.root.iterdir()), [self.root / "report.json"])

    def test_missing_commands_explicit_before_credentials(self):
        output = self.root / "report.json"
        with (
            patch.dict(os.environ, {"EXCLUSIVE_DISPOSABLE_1ES": "true"}),
            patch.object(inspector.shutil, "which", return_value=None),
            patch.object(inspector, "command") as run,
        ):
            self.assertEqual(inspector.execute(output, self.root), 1)
        run.assert_not_called()
        self.assertIn(
            "command:guestfish", json.loads(output.read_text())["unavailable"]
        )

    def test_fixed_cohort_and_no_key_artifact(self):
        self.assertEqual(set(inspector.ARTIFACT_IDS), set(inspector.prepare.STAGES))
        self.assertEqual(inspector.BASE["version"], "3.0.20261116")
        self.assertNotIn("*", inspector.BASE["selectedFile"])

    def test_exact_cohort_both_clones_tools_and_immutable_base_orchestration(self):
        names = {}
        listing = []
        for name, artifact_id in inspector.ARTIFACT_IDS.items():
            if name == "go-tools":
                names[name] = inspector.prepare.TOOLS
            elif name == "trident-binaries":
                names[name] = (
                    "trident-0.31.0-1.azl3.x86_64.rpm",
                    "trident-service-0.31.0-1.azl3.x86_64.rpm",
                )
            elif name in inspector.prepare.INSTALLERS:
                names[name] = (name + ".iso",)
            else:
                names[name] = tuple(f"{name}_{index}.cosi" for index in range(2))
            listing.append(
                {
                    "id": artifact_id,
                    "name": name,
                    "resource": {
                        "type": "PipelineArtifact",
                        "properties": {"artifactsize": "123"},
                        "downloadUrl": f"https://fixture.artifacts.visualstudio.com/{name}",
                    },
                }
            )

        downloaded = []

        def download(url, token, target):
            name = url.rsplit("/", 1)[1]
            downloaded.append(name)
            with zipfile.ZipFile(target, "w") as archive:
                for filename in names[name]:
                    archive.writestr(
                        name + "/" + filename, ("fixture:" + filename).encode()
                    )

        def run(argv, **kwargs):
            if argv[:4] == ["az", "artifacts", "universal", "download"]:
                self.assertEqual(
                    argv[4:-3],
                    [
                        "--organization",
                        "https://dev.azure.com/mariner-org",
                        "--project",
                        "36d030d6-1d99-4ebd-878b-09af1f4f722f",
                        "--scope",
                        "project",
                        "--feed",
                        "AzureLinuxArtifacts",
                        "--name",
                        "baremetal_vhdx-3.0-stable",
                        "--version",
                        "3.0.20261116",
                        "--file-filter",
                        "core-3.0.20261116.vhdx",
                    ],
                )
                self.assertEqual(argv[-3], "--path")
                self.assertEqual(argv[-1], "--only-show-errors")
                self.assertEqual(
                    kwargs,
                    {
                        "timeout": 1200,
                        "failure_fact": "exact-base-universal-download-failed",
                    },
                )
                self.assertEqual(argv[argv.index("--version") + 1], "3.0.20261116")
                self.assertEqual(
                    argv[argv.index("--file-filter") + 1],
                    inspector.BASE["selectedFile"],
                )
                base = Path(argv[argv.index("--path") + 1])
                (base / inspector.BASE["selectedFile"]).write_bytes(
                    b"fixture exact base bytes"
                )
                return ""
            self.assertEqual(argv[:3], ["az", "account", "get-access-token"])
            return "fixture-token"

        facts = {}
        with (
            patch.object(inspector, "command", side_effect=run),
            patch.object(
                inspector.prepare,
                "verify_server",
                return_value=(
                    {"definition": {"revision": 7}},
                    [],
                    {},
                    [],
                ),
            ) as server,
            patch.object(inspector, "collect_logs", return_value={}),
            patch.object(
                inspector.ado, "request_json", return_value={"value": listing}
            ),
            patch.object(inspector.ado, "download_file", side_effect=download),
            patch.object(inspector, "source_rpms", return_value=self.packages),
            patch.object(
                inspector,
                "inspect_image",
                return_value={
                    "basis": "fixture-only-not-producer-evidence",
                },
            ) as inspect,
        ):
            inspector.inspect(self.root, facts)
        self.assertEqual(downloaded, list(inspector.ARTIFACT_IDS))
        self.assertEqual(server.call_args.args[1]["sourceVersion"], inspector.REVISION)
        self.assertEqual(inspect.call_count, 8)
        self.assertEqual(
            sum(
                len(row["files"])
                for name, row in facts["artifacts"].items()
                if name != "trident-binaries"
            ),
            13,
        )
        self.assertEqual(
            facts["baseVhdx"]["sha256"],
            hashlib.sha256(b"fixture exact base bytes").hexdigest(),
        )

    def test_selected_original_log_hash_count_no_body_or_free_selector(self):
        body = b"fixture original line one\r\nfixture original line two\r\n"

        class Response(io.BytesIO):
            class headers:
                @staticmethod
                def get_content_type():
                    return "text/plain"

        records = [
            {
                "id": "safe-task",
                "type": "Task",
                "state": "completed",
                "result": "succeeded",
                "parentId": "stage",
                "log": {"id": 58},
            },
            {"id": "stage"},
        ]
        with (
            patch.object(inspector, "SOURCE_LOGS", {"go-tools": (58,)}),
            patch.object(inspector, "INPUT_CITES", {}),
            patch.object(inspector.urllib.request, "build_opener") as opener,
        ):
            opener.return_value.open.side_effect = lambda *a, **k: contextlib.closing(
                Response(body)
            )
            result = inspector.collect_logs(
                "fixture-token",
                records,
                {inspector.prepare.STAGES["go-tools"]: "stage"},
                [{"id": 58, "lineCount": 2}],
            )
        self.assertEqual(
            result, {"58": {"sha256": hashlib.sha256(body).hexdigest(), "lineCount": 2}}
        )
        self.assertNotIn("line one", json.dumps(result))

    def test_wrong_log_ancestry_fails_before_download(self):
        with (
            patch.object(inspector.urllib.request, "build_opener") as opener,
            self.assertRaisesRegex(inspector.Failure, "log-task-identity"),
        ):
            inspector.collect_logs("fixture-token", [], {}, [])
        opener.assert_not_called()

    def test_real_command_failure_has_no_console_body(self):
        with self.assertRaisesRegex(inspector.Failure, "^inspection-command-failed$"):
            inspector.command(
                [
                    sys.executable,
                    "-c",
                    "print('fixture-sensitive-body'); raise SystemExit(4)",
                ]
            )

    def test_base_download_stderr_classification_is_allowlisted_not_a_root_cause(self):
        examples = (
            (
                b"Before you can run Azure DevOps commands, you need to run the login command",
                "authentication-message",
            ),
            (b"HTTP 401 Unauthorized", "access-message"),
            (b"HTTPError: 403 Forbidden", "http-403"),
            (
                b"unrecognized arguments: --file-filter",
                "cli-contract-or-version-message",
            ),
            (
                b"Failed to update Universal Packages tooling",
                "artifacttool-bootstrap-message",
            ),
            (
                b"You must install or update .NET to run this application",
                "artifacttool-runtime-message",
            ),
            (b"Cannot find the package", "package-or-feed-message"),
            (
                b"CERTIFICATE_VERIFY_FAILED: certificate verify failed",
                "transport-message",
            ),
            (b"\xff\x00fixture-sensitive-unknown", "unclassified"),
        )
        for body, marker in examples:
            with (
                self.subTest(marker=marker),
                patch.object(
                    inspector,
                    "base_cli_versions",
                    return_value={"status": "unavailable"},
                ),
            ):
                diagnostic = inspector.base_download_diagnostic(
                    7,
                    io.BytesIO(
                        body
                        + b"\nBearer fixture-sensitive-token https://fixture.invalid/?sig=fixture-sensitive"
                    ),
                )
            self.assertIn(marker, diagnostic["stderrMarkers"])
            self.assertEqual(diagnostic["exitCode"], 7)
            self.assertEqual(
                diagnostic["basis"], "bounded-stderr-markers-not-root-cause"
            )
            self.assertNotIn("fixture-sensitive", json.dumps(diagnostic))
            self.assertNotIn("fixture.invalid", json.dumps(diagnostic))

    def test_base_download_diagnostic_reads_only_bounded_private_prefix(self):
        stderr = io.BytesIO(b"x" * (64 * 1024) + b"HTTP 403 Forbidden")
        with (
            patch.object(
                inspector, "base_cli_versions", return_value={"status": "unavailable"}
            ),
            patch.object(stderr, "read", wraps=stderr.read) as read,
        ):
            diagnostic = inspector.base_download_diagnostic(1, stderr)
        read.assert_called_once_with(64 * 1024 + 1)
        self.assertTrue(diagnostic["stderrTruncated"])
        self.assertEqual(diagnostic["stderrMarkers"], ["unclassified"])

    def test_base_cli_versions_emit_only_numeric_allowlisted_metadata(self):
        with patch.object(
            inspector,
            "command",
            return_value=json.dumps(
                {
                    "azure-cli": "2.77.0",
                    "extensions": {"azure-devops": "1.0.2"},
                    "unknown": "fixture-sensitive",
                }
            ),
        ) as run:
            self.assertEqual(
                inspector.base_cli_versions(),
                {
                    "status": "available",
                    "azureCli": "2.77.0",
                    "azureDevopsExtension": "1.0.2",
                },
            )
        run.assert_called_once_with(
            ["az", "version", "--output", "json"],
            timeout=30,
            failure_fact="base-cli-version-unavailable",
        )
        for value in (
            "not-json fixture-sensitive",
            "[]",
            json.dumps(
                {
                    "azure-cli": "fixture-sensitive-token",
                    "extensions": {
                        "azure-devops": "https://fixture.invalid/?sig=fixture-sensitive"
                    },
                }
            ),
        ):
            with patch.object(inspector, "command", return_value=value):
                versions = inspector.base_cli_versions()
            self.assertIsNone(versions["azureCli"])
            self.assertIsNone(versions["azureDevopsExtension"])
            self.assertEqual(versions["status"], "unavailable")
            self.assertNotIn("fixture-sensitive", json.dumps(versions))

    def test_base_download_primary_exit_survives_secondary_metadata_failures(self):
        real_command = inspector.command
        errors = (
            inspector.Failure("fixture-sensitive"),
            OSError("fixture-sensitive"),
            subprocess.TimeoutExpired("fixture-sensitive", 30),
            inspector.Cancelled(),
        )
        for error in errors:

            def invoke(argv, **kwargs):
                if argv == ["az", "version", "--output", "json"]:
                    raise error
                self.assertEqual(argv, [sys.executable, "-c", "raise SystemExit(17)"])
                return real_command(argv, **kwargs)

            with (
                self.subTest(error=type(error).__name__),
                patch.object(inspector, "command", side_effect=invoke),
                self.assertRaisesRegex(
                    inspector.Failure, "^exact-base-universal-download-failed$"
                ) as caught,
            ):
                inspector.command(
                    [sys.executable, "-c", "raise SystemExit(17)"],
                    failure_fact="exact-base-universal-download-failed",
                )
            diagnostic = caught.exception.diagnostic
            self.assertEqual(diagnostic["exitCode"], 17)
            self.assertEqual(
                diagnostic["versions"]["status"],
                (
                    "cancelled"
                    if isinstance(error, inspector.Cancelled)
                    else "unavailable"
                ),
            )
            self.assertNotIn("fixture-sensitive", json.dumps(diagnostic))

    def test_base_download_stderr_read_failure_preserves_exit_without_raw_error(self):
        stderr = io.BytesIO()
        with (
            patch.object(
                inspector, "base_cli_versions", return_value={"status": "unavailable"}
            ),
            patch.object(stderr, "read", side_effect=OSError("fixture-sensitive")),
        ):
            diagnostic = inspector.base_download_diagnostic(-9, stderr)
        self.assertEqual(diagnostic["exitCode"], -9)
        self.assertEqual(diagnostic["stderrStatus"], "unavailable")
        self.assertIsNone(diagnostic["stderrTruncated"])
        self.assertNotIn("fixture-sensitive", json.dumps(diagnostic))

    def test_base_diagnostics_only_run_for_exact_download_nonzero_exit(self):
        with patch.object(inspector, "base_download_diagnostic") as diagnostic:
            self.assertEqual(
                inspector.command(
                    [sys.executable, "-c", "print('fixture')"],
                    failure_fact="exact-base-universal-download-failed",
                ),
                "fixture\n",
            )
            with self.assertRaisesRegex(
                inspector.Failure, "^inspection-command-failed$"
            ):
                inspector.command([sys.executable, "-c", "raise SystemExit(4)"])
            with self.assertRaises(subprocess.TimeoutExpired):
                inspector.command(
                    [sys.executable, "-c", "import time; time.sleep(20)"],
                    timeout=0.1,
                    failure_fact="exact-base-universal-download-failed",
                )
        diagnostic.assert_not_called()

    def test_real_base_failure_report_preserves_partial_facts_and_redacts_all_output(
        self,
    ):
        with (
            patch.object(
                inspector, "base_cli_versions", return_value={"status": "unavailable"}
            ),
            self.assertRaises(inspector.Failure) as caught,
        ):
            inspector.command(
                [
                    sys.executable,
                    "-c",
                    "import sys; print('fixture-sensitive-stdout'); "
                    "sys.stderr.write('Failed to update Universal Packages tooling HTTP 403 Forbidden "
                    "Bearer fixture-sensitive-token https://fixture.invalid/?sig=fixture-sensitive'); "
                    "raise SystemExit(17)",
                ],
                failure_fact="exact-base-universal-download-failed",
            )

        def partial(work, facts):
            facts["rpms"] = self.packages
            raise caught.exception

        code, raw, report = self.execute_fixture(partial)
        self.assertEqual(code, 1)
        self.assertEqual(report["status"], "failure")
        self.assertEqual(report["reviewStatus"], "UNREVIEWED")
        self.assertEqual(report["complete"]["rpms"], self.packages)
        self.assertNotIn("baseVhdx", report["complete"])
        self.assertEqual(
            report["failure"]["missingFact"], "exact-base-universal-download-failed"
        )
        self.assertEqual(report["failure"]["diagnostic"]["exitCode"], 17)
        self.assertEqual(
            report["failure"]["diagnostic"]["stderrMarkers"],
            [
                "access-message",
                "http-403",
                "artifacttool-bootstrap-message",
            ],
        )
        self.assertNotIn("fixture-sensitive", raw)
        self.assertNotIn("fixture.invalid", raw)
        self.assertNotIn("Bearer", raw)

    def test_real_command_timeout_reaps_only_owned_process(self):
        processes = []
        original = subprocess.Popen

        def capture(*args, **kwargs):
            process = original(*args, **kwargs)
            processes.append(process)
            return process

        with patch.object(inspector.subprocess, "Popen", side_effect=capture):
            with self.assertRaises(subprocess.TimeoutExpired):
                inspector.command(
                    [sys.executable, "-c", "import time; time.sleep(20)"], timeout=0.1
                )
        self.assertIsNotNone(processes[0].poll())


if __name__ == "__main__":
    unittest.main()
