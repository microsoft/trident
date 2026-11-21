import hashlib
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

from pathlib import Path

SETUP = Path(__file__).resolve().parents[1] / "setup-functional.sh"

MOCKS = r"""
set -euo pipefail
readonly MOCK_ROOT="$PWD/${BASH_SOURCE[0]%/*}"
export HOME="$MOCK_ROOT/home"
export USER="setup-test"
export MOCK_LOG="$MOCK_ROOT/commands.log"
mkdir -p "$HOME"
mountpoint() { [[ "$*" == "--quiet /mnt/storage" ]]; }
id() { printf '%s\n' "$MOCK_UID"; }
sudo() { printf 'sudo %s\n' "$*" >> "$MOCK_LOG"; }
python3() { printf 'python3 %s\n' "$*" >> "$MOCK_LOG"; }
ssh-keygen() {
    if [[ "$MOCK_SSH_KEYGEN" == "real" ]]; then
        printf 'ssh-keygen\n' >> "$MOCK_LOG"
        builtin command ssh-keygen "$@"
        return
    fi
    echo "Unexpected key generation" >&2
    return 90
}
command() {
    if [[ "$*" == "-v xorrisofs" ]]; then
        [[ "$MOCK_XORRISOFS" == "present" ]]
        return
    fi
    builtin command "$@"
}
test() {
    if [[ "$*" == "-e /dev/kvm" ]]; then return 0; fi
    builtin test "$@"
}
virsh() {
    [[ "$LIBVIRT_DEFAULT_URI" == "qemu:///system" ]]
    [[ "$*" == "list --all" ]]
    printf 'virsh %s %s\n' "$LIBVIRT_DEFAULT_URI" "$*" >> "$MOCK_LOG"
}
sg() {
    [[ "$1" == "libvirt" && "$2" == "-c" ]]
    eval "$3"
}
export LIBVIRT_DEFAULT_URI="test:///default"
MOCK_XORRISOFS="$1"
MOCK_UID="${2:-1000}"
MOCK_SSH_KEYGEN="${3:-unexpected}"
"""


@unittest.skipUnless(shutil.which("bash"), "requires bash")
class FunctionalSetupTests(unittest.TestCase):
    @unittest.skipUnless(hasattr(os, "getuid"), "requires Unix file ownership")
    def test_lock_provisioning_preserves_inode_content_and_existing_lock(self):
        import fcntl

        source = SETUP.read_text()
        start = source.index(
            'sudo install -d -m 0750 -o root -g libvirt "$FUNCTIONAL_LOCK_DIR"'
        )
        end = source.index("sudo install -d \\", start + 1)
        provision = source[start:end]
        # Exercise the real provisioning commands within a disposable directory,
        # mapping root/libvirt to the test account instead of changing the host.
        uid, gid = os.getuid(), os.getgid()
        provision = provision.replace("-o root -g libvirt", f"-o {uid} -g {gid}")
        provision = provision.replace("chown root:libvirt", f"chown {uid}:{gid}")
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            directory = root / "locks"
            script = root / "provision.sh"
            script.write_text(
                'set -euo pipefail\nFUNCTIONAL_LOCK_DIR="$1"\nsudo() { "$@"; }\n'
                + provision,
                newline="\n",
            )
            command = ["bash", str(script), str(directory)]
            subprocess.run(command, capture_output=True, text=True, check=True)
            lock = directory / "functional.lock"
            lock.write_text("existing lock content")
            identity = lock.stat()
            with lock.open("r") as held:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                subprocess.run(command, capture_output=True, text=True, check=True)
                contender = subprocess.run(
                    ["flock", "-n", str(lock), "true"], capture_output=True
                )
                self.assertNotEqual(contender.returncode, 0)
            self.assertEqual(lock.read_text(), "existing lock content")
            self.assertEqual(lock.stat().st_ino, identity.st_ino)
            for path, mode in ((directory, 0o750), (lock, 0o640)):
                info = path.stat()
                self.assertEqual(stat.S_IMODE(info.st_mode), mode)
                self.assertEqual((info.st_uid, info.st_gid), (uid, gid))
            lock.unlink()
            victim = root / "victim"
            victim.write_text("untouched")
            victim.chmod(0o600)
            lock.symlink_to(victim)
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Unsafe functional lock", result.stderr)
            self.assertEqual(victim.read_text(), "untouched")
            self.assertEqual(stat.S_IMODE(victim.stat().st_mode), 0o600)

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp_dir.cleanup)
        self.root = Path(self.temp_dir.name)
        self.home = self.root / "home"
        ssh = self.home / ".ssh"
        ssh.mkdir(parents=True)
        self.keys = {
            ssh / "id_rsa": b"existing private key placeholder\n",
            ssh / "id_rsa.pub": b"existing public key placeholder\n",
        }
        for path, content in self.keys.items():
            path.write_bytes(content)
        self.config = self.home / ".config" / "libvirt" / "libvirt.conf"
        self.script = self.root / "setup-test.sh"
        self.script.write_text(MOCKS + SETUP.read_text(), newline="\n")

    def run_setup(self, executable="present", uid=1000, ssh_keygen="unexpected"):
        return subprocess.run(
            [
                "bash",
                f"{self.root.name}/{self.script.name}",
                executable,
                str(uid),
                ssh_keygen,
            ],
            cwd=self.root.parent,
            capture_output=True,
            text=True,
            timeout=15,
        )

    def test_setup_provisions_separate_cache_roots_for_each_uid(self):
        for uid in (1000, 2000, 1000):
            result = self.run_setup(uid=uid)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = (self.root / "commands.log").read_text().splitlines()
        cache_installs = [
            line for line in commands if line.startswith("sudo install -d -m 0755")
        ]
        self.assertEqual(
            cache_installs,
            [
                f"sudo install -d -m 0755 -o {uid} -g {uid} "
                f"/mnt/storage/trident-cloud-agent/functional-{uid}"
                for uid in (1000, 2000, 1000)
            ],
        )

    def generate_test_key(self, passphrase=""):
        private, public = self.keys
        private.unlink()
        public.unlink()
        result = subprocess.run(
            [
                "bash",
                "-c",
                'ssh-keygen -t ed25519 -q -N "$1" -f "$2"',
                "bash",
                passphrase,
                f"{self.root.name}/home/.ssh/id_rsa",
            ],
            cwd=self.root.parent,
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        expected_public = public.read_text().split()[:2]
        public.unlink()
        return private, public, expected_public

    @unittest.skipUnless(shutil.which("ssh-keygen"), "requires ssh-keygen")
    def test_setup_derives_missing_public_key_and_preserves_private_key(self):
        private, public, expected = self.generate_test_key()
        digest = hashlib.sha256(private.read_bytes()).hexdigest()
        for _ in range(2):
            result = self.run_setup(ssh_keygen="real")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue(public.is_file())
            self.assertEqual(public.read_text().split()[:2], expected)
            self.assertEqual(hashlib.sha256(private.read_bytes()).hexdigest(), digest)
            self.assertNotIn("PRIVATE KEY", result.stdout + result.stderr)
        commands = (self.root / "commands.log").read_text().splitlines()
        self.assertEqual(commands.count("ssh-keygen"), 1)

    @unittest.skipUnless(shutil.which("ssh-keygen"), "requires ssh-keygen")
    def test_setup_rejects_encrypted_key_without_leaving_public_key(self):
        private, public, _ = self.generate_test_key("test-only-passphrase")
        digest = hashlib.sha256(private.read_bytes()).hexdigest()
        for _ in range(2):
            result = self.run_setup(ssh_keygen="real")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(
                "Cannot derive SSH public key noninteractively", result.stderr
            )
            self.assertFalse(public.exists())
            self.assertEqual(hashlib.sha256(private.read_bytes()).hexdigest(), digest)
            self.assertNotIn("PRIVATE KEY", result.stdout + result.stderr)

    def test_repeated_setup_preserves_config_and_keys_and_installs_xorriso(self):
        self.config.parent.mkdir(parents=True)
        original = b'# developer settings\r\nuri_default = "test:///default"\r\n'
        self.config.write_bytes(original)

        for _ in range(2):
            result = self.run_setup()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(self.config.read_bytes(), original)
            for path, content in self.keys.items():
                self.assertEqual(path.read_bytes(), content)

        commands = (self.root / "commands.log").read_text().splitlines()
        installs = [line for line in commands if "apt-get install" in line]
        self.assertEqual(len(installs), 2)
        for install in installs:
            self.assertIn("xorriso", install.split())
        self.assertEqual(commands.count("virsh qemu:///system list --all"), 2)
        self.assertEqual(commands.count("sudo usermod -a -G libvirt setup-test"), 2)
        self.assertEqual(commands.count("sudo systemctl start libvirtd.service"), 2)

    def test_setup_does_not_create_libvirt_config(self):
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.config.parent.exists())

    def test_setup_fails_explicitly_when_xorrisofs_is_missing(self):
        result = self.run_setup(executable="missing")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "Missing xorrisofs executable (provided by xorriso)", result.stderr
        )
        commands = (self.root / "commands.log").read_text()
        self.assertNotIn("usermod", commands)
        self.assertNotIn("systemctl", commands)
