import shutil
import subprocess
import tempfile
import unittest

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

RUNNER = Path(__file__).resolve().parents[1] / "run-functional.sh"


@unittest.skipUnless(shutil.which("bash"), "requires bash")
class FunctionalRunnerTests(unittest.TestCase):
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
            script.write_text(
                source + r"""
enter_libvirt_group() { :; }
az() { :; }
ip() { :; }
virsh() { :; }
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
                    capture_output=True,
                    text=True,
                )

            with ThreadPoolExecutor(max_workers=2) as executor:
                results = list(executor.map(invoke, (0, 1)))
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
