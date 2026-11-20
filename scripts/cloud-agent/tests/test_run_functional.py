import os
import shutil
import stat
import subprocess
import tempfile
import time
import unittest

from pathlib import Path

RUNNER = Path(__file__).resolve().parents[1] / "run-functional.sh"


@unittest.skipUnless(shutil.which("bash"), "requires bash")
class FunctionalRunnerTests(unittest.TestCase):
    def lock_script(self, root, extra=""):
        script_dir = root / "scripts/cloud-agent"
        script_dir.mkdir(parents=True)
        lock_dir = root / "locks"
        lock_dir.mkdir()
        lock_dir.chmod(0o750)
        root.chmod(0o755)
        lock = lock_dir / "host.lock"
        lock.touch(mode=0o640)
        source, _ = RUNNER.read_text().rsplit('\nmain "$@"', 1)
        source = source.replace(
            "/var/lib/trident-cloud-agent/functional.lock", str(lock)
        )
        script = script_dir / "run-functional.sh"
        script.write_text(
            source
            + "\n"
            + extra
            + '\nacquire_host_lock\nprintf "locked\\n"\n'
            + 'if [[ "${1:-}" == hold ]]; then read -r release; fi\n',
            newline="\n",
        )
        return script, lock

    @unittest.skipUnless(shutil.which("flock"), "requires flock")
    def test_lock_rejects_nonregular_or_linked_files(self):
        for kind in ("symlink", "hardlink", "fifo"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp_dir:
                root = Path(temp_dir)
                script, lock = self.lock_script(root)
                target = root / "target"
                target.write_text("untouched")
                lock.unlink()
                if kind == "symlink":
                    lock.symlink_to(target)
                elif kind == "hardlink":
                    os.link(target, lock)
                else:
                    os.mkfifo(lock)
                result = subprocess.run(
                    ["bash", str(script)], capture_output=True, text=True, timeout=5
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(target.read_text(), "untouched")

    @unittest.skipUnless(shutil.which("flock"), "requires flock")
    def test_lock_rejects_inode_replacement_after_acquisition(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            script, lock = self.lock_script(
                root,
                r"""
flock() {
    command flock "$@" || return
    mv "$REPO_ROOT/replacement" "$REPO_ROOT/locks/host.lock"
}
""",
            )
            (root / "replacement").touch()
            result = subprocess.run(
                ["bash", str(script)], capture_output=True, text=True, timeout=5
            )
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn("locked", result.stdout)

    @unittest.skipUnless(
        hasattr(os, "geteuid") and os.geteuid() == 0 and shutil.which("setpriv"),
        "requires root and setpriv for real cross-UID validation",
    )
    def test_provisioned_lock_reopens_across_uids_with_protected_regular(self):
        protected = Path("/proc/sys/fs/protected_regular")
        if not protected.exists() or int(protected.read_text()) == 0:
            self.skipTest("requires fs.protected_regular")
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            script, lock = self.lock_script(root)
            os.chown(lock.parent, 0, 65531)
            os.chown(lock, 0, 65531)

            def command(uid, *args):
                return [
                    "setpriv",
                    f"--reuid={uid}",
                    f"--regid={uid}",
                    "--groups=65531",
                    "bash",
                    str(script),
                    *args,
                ]

            first = subprocess.run(
                command(65532), capture_output=True, text=True, timeout=5
            )
            self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
            identity = lock.stat()
            self.assertEqual(stat.S_IMODE(identity.st_mode), 0o640)
            second = subprocess.run(
                command(65533), capture_output=True, text=True, timeout=5
            )
            self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
            holder = subprocess.Popen(
                command(65532, "hold"),
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            try:
                self.assertEqual(holder.stdout.readline(), "locked\n")
                busy = subprocess.run(
                    command(65533), capture_output=True, text=True, timeout=5
                )
                self.assertNotEqual(busy.returncode, 0)
                self.assertIn("another functional run", busy.stderr)
            finally:
                stdout, stderr = holder.communicate("release\n", timeout=5)
            self.assertEqual(holder.returncode, 0, stdout + stderr)
            final = lock.stat()
            self.assertEqual(
                (final.st_dev, final.st_ino), (identity.st_dev, identity.st_ino)
            )
            self.assertEqual(final.st_uid, 0)
            self.assertEqual(final.st_gid, 65531)

    def test_runs_isolate_vm_cache_and_remove_it_on_success_or_failure(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            script_dir = root / "scripts" / "cloud-agent"
            script_dir.mkdir(parents=True)
            (root / "tools").mkdir()
            (root / "bin").mkdir()
            (root / "image.qcow2").touch()
            shared_cache = root / ".pytest_cache"
            shared_cache.mkdir()
            sentinel = shared_cache / "existing-vm"
            sentinel.write_text("developer-vm")
            virtdeploy = root / "bin" / "virtdeploy"
            virtdeploy.write_text("#!/usr/bin/env bash\nexit 0\n", newline="\n")
            virtdeploy.chmod(0o755)
            script = script_dir / "run-functional.sh"
            source, entrypoint = RUNNER.read_text().rsplit('\nmain "$@"', 1)
            self.assertFalse(entrypoint.strip())
            source = source.replace(
                "/var/lib/trident-cloud-agent/functional.lock", str(root / "host.lock")
            )
            (root / "host.lock").touch(mode=0o640)
            script.write_text(
                source
                + r"""
enter_libvirt_group() { :; }
az() { :; }
ip() { :; }
virsh() { [[ "$LIBVIRT_DEFAULT_URI" == qemu:///system ]]; }
qemu-system-x86_64() { :; }
go() { :; }
export -f go
make() { :; }
test() {
    if [[ "$*" == "-e /dev/kvm" ]]; then return 0; fi
    builtin test "$@"
}
python3() {
    case "$1" in
        *download_functional_artifact.py)
            printf '%s\n' "$REPO_ROOT/image.qcow2"
            ;;
        *select_functional_network.py)
            printf '192.168.50.0/24\n'
            ;;
        *)
            local cache=""
            local argument
            for argument in "$@"; do
                case "$argument" in cache_dir=*) cache="${argument#cache_dir=}";; esac
            done
            [[ -n "$cache" && -d "$cache" ]] || return 90
            printf '%s\n' "$cache" > "$REPO_ROOT/$TRIDENT_FUNCTIONAL_NAMESPACE.cache-path"
            printf '%s\n' "$TRIDENT_FUNCTIONAL_NAMESPACE" > "$cache/vm_ssh_node"
            sleep 0.2
            [[ "$(cat "$cache/vm_ssh_node")" == "$TRIDENT_FUNCTIONAL_NAMESPACE" ]] || return 91
            return "$MOCK_PYTEST_EXIT"
            ;;
    esac
}
export TRIDENT_FUNCTIONAL_NAMESPACE="run-$1"
export MOCK_PYTEST_EXIT="$1"
export XDG_RUNTIME_DIR="."
main smoke
""",
                newline="\n",
            )

            def invoke(run):
                return subprocess.run(
                    ["bash", "scripts/cloud-agent/run-functional.sh", str(run)],
                    cwd=root,
                    env=dict(os.environ, LIBVIRT_DEFAULT_URI="qemu:///session"),
                    capture_output=True,
                    text=True,
                )

            results = [invoke(run) for run in (0, 1)]
            for code, result in enumerate(results):
                self.assertEqual(result.returncode, code, result.stdout + result.stderr)
            cache_paths = [
                (root / f"run-{run}.cache-path").read_text().strip() for run in (0, 1)
            ]
            self.assertNotEqual(*cache_paths)
            for cache_path in cache_paths:
                result = subprocess.run(
                    ["bash", "-c", 'test ! -e "$1"', "bash", cache_path]
                )
                self.assertEqual(result.returncode, 0)
            self.assertEqual(sentinel.read_text(), "developer-vm")

    @unittest.skipUnless(shutil.which("flock"), "requires flock")
    def test_host_lock_covers_generators_and_cleanup_across_workspaces(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            shared = Path(temp_dir)
            source, _ = RUNNER.read_text().rsplit('\nmain "$@"', 1)
            source = source.replace(
                "/var/lib/trident-cloud-agent/functional.lock",
                str(shared / "host.lock"),
            )
            (shared / "host.lock").touch(mode=0o640)
            for name in ("foo", "foo-nvram"):
                root = shared / name
                script_dir = root / "scripts/cloud-agent"
                script_dir.mkdir(parents=True)
                (root / "tools").mkdir()
                (root / "bin").mkdir()
                (root / "image.qcow2").touch()
                (root / "home").mkdir()
                (root / "bin/virtdeploy").write_text(
                    "#!/usr/bin/env bash\n"
                    'touch "$SHARED/cleanup"\n'
                    "for ((i=0; i<500; i++)); do\n"
                    '  [[ -e "$SHARED/release-cleanup" ]] && exit 0\n'
                    "  sleep 0.01\n"
                    "done\nexit 99\n",
                    newline="\n",
                )
                (root / "bin/virtdeploy").chmod(0o755)
                (script_dir / "run-functional.sh").write_text(
                    source
                    + r"""
enter_libvirt_group() { :; }
az() { :; }
ip() { :; }
virsh() { :; }
qemu-system-x86_64() { :; }
test() {
    if [[ "$*" == "-e /dev/kvm" ]]; then return 0; fi
    builtin test "$@"
}
make() { :; }
go() {
    touch "$SHARED/generate"
    for ((i=0; i<500; i++)); do
        [[ -e "$SHARED/release-generate" ]] && return 0
        sleep 0.01
    done
    return 99
}
export -f go
python3() {
    touch "$REPO_ROOT/mutated"
    case "$1" in
        *download_functional_artifact.py) printf '%s\n' "$REPO_ROOT/image.qcow2";;
        *select_functional_network.py) printf '192.168.50.0/24\n';;
        *) [[ "$LIBVIRT_DEFAULT_URI" == qemu:///system ]];;
    esac
}
export SHARED="$REPO_ROOT/.."
export HOME="$REPO_ROOT/home"
export XDG_RUNTIME_DIR="$HOME"
export TRIDENT_FUNCTIONAL_NAMESPACE="${REPO_ROOT##*/}"
main smoke
""",
                    newline="\n",
                )

            def invoke(root):
                return subprocess.Popen(
                    ["bash", "scripts/cloud-agent/run-functional.sh"],
                    cwd=root,
                    env=dict(os.environ, LIBVIRT_DEFAULT_URI="qemu:///session"),
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )

            def wait_for(stage, process):
                deadline = time.monotonic() + 10
                while not (shared / stage).exists():
                    if process.poll() is not None:
                        self.fail(str(process.communicate()))
                    self.assertLess(time.monotonic(), deadline, stage)
                    time.sleep(0.01)

            first = invoke(shared / "foo")
            try:
                # The mock also proves an inherited session URI is overridden.
                wait_for("generate", first)
                for stage in ("generate", "cleanup"):
                    if stage == "cleanup":
                        (shared / "release-generate").touch()
                        wait_for("cleanup", first)
                    second = invoke(shared / "foo-nvram")
                    stdout, stderr = second.communicate(timeout=10)
                    self.assertNotEqual(second.returncode, 0, stdout + stderr)
                    self.assertIn("another functional run", stderr)
                    self.assertFalse((shared / "foo-nvram/mutated").exists())
            finally:
                (shared / "release-generate").touch()
                (shared / "release-cleanup").touch()
                stdout, stderr = first.communicate(timeout=10)
            self.assertEqual(first.returncode, 0, stdout + stderr)
