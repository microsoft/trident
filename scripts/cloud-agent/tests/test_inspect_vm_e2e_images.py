import hashlib
import contextlib
import importlib.util
import io
import json
import os
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
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
            ["true\ntrue\n", "false\n", "", (self.digest + "\n") * count]
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
                self.inspect_fixture(["true\ntrue\n", "false\n", ""], installed)

    def test_missing_rpmdb_or_live_wal_fails(self):
        for answers in (["false\nfalse\n"], ["true\ntrue\n", "true\n"]):
            with self.subTest(answers=answers), self.assertRaises(inspector.Failure):
                self.inspect_fixture(answers)

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
            self.inspect_fixture(["true\ntrue\n", "false\n", "", ("0" * 64 + "\n") * 4])
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
