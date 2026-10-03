#!/usr/bin/env python3
import argparse
import contextlib
import json
import os
from pathlib import Path
import select
import shlex
import shutil
import socket
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent
QMP_STARTUP_TIMEOUT_SECONDS = 30
CODE_CANDIDATES = [
    Path("/usr/share/OVMF/OVMF_CODE_4M.fd"),
    Path("/usr/share/OVMF/OVMF_CODE.fd"),
    Path("/usr/share/edk2/ovmf/OVMF_CODE.fd"),
    Path("/usr/share/qemu/edk2-x86_64-code.fd"),
]


def command(iso, scratch, code, memory, cpus, display, vnc=None):
    acceleration = (
        ["-enable-kvm", "-cpu", "host"] if os.access("/dev/kvm", os.W_OK) else []
    )
    launch = [
        "qemu-system-x86_64",
        *acceleration,
        "-m",
        str(memory),
        "-smp",
        str(cpus),
        "-machine",
        "q35",
        "-drive",
        f"if=pflash,format=raw,unit=0,readonly=on,file={code}",
        "-drive",
        f"if=pflash,format=raw,unit=1,file={scratch / 'vars.fd'}",
        "-drive",
        f"if=none,id=hd0,file={scratch / 'disk.qcow2'},format=qcow2",
        "-device",
        "virtio-blk-pci,drive=hd0,bootindex=1",
        "-drive",
        f"if=none,id=cd0,file={iso},media=cdrom,readonly=on",
        "-device",
        "ide-cd,bus=ide.0,drive=cd0,bootindex=2",
        "-nic",
        "user,model=virtio-net-pci",
        "-display",
        "none" if vnc is not None else display,
        "-serial",
        f"file:{scratch / 'serial.log'}",
    ]
    if vnc is not None:
        launch.extend(
            [
                "-S",
                "-qmp",
                f"unix:{scratch / 'monitor.sock'},server=on,wait=off",
                "-vnc",
                f"127.0.0.1:{vnc}",
            ]
        )
    return launch


class Qmp:
    def __init__(self, connection, process):
        self.connection = connection
        self.process = process
        self.pending = b""
        self.viewer_connected = False

    def receive(self):
        while b"\n" not in self.pending:
            if self.process.poll() is not None:
                raise RuntimeError("QEMU exited while waiting for a VNC viewer")
            if not select.select([self.connection], [], [], 1)[0]:
                continue
            data = self.connection.recv(4096)
            if not data:
                raise RuntimeError("QEMU monitor disconnected")
            self.pending += data
        line, self.pending = self.pending.split(b"\n", 1)
        message = json.loads(line)
        if message.get("event") == "VNC_INITIALIZED":
            self.viewer_connected = True
        elif message.get("event") == "VNC_DISCONNECTED":
            self.viewer_connected = False
        return message

    def execute(self, command):
        request = {"execute": command, "id": command}
        self.connection.sendall(json.dumps(request).encode() + b"\n")
        while True:
            response = self.receive()
            if response.get("id") == command:
                if "error" in response:
                    raise RuntimeError(f"QEMU {command}: {response['error']}")
                return response["return"]


def wait_for_vnc_viewer(process, monitor_path, on_ready):
    deadline = time.monotonic() + QMP_STARTUP_TIMEOUT_SECONDS
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        while True:
            if process.poll() is not None:
                raise RuntimeError("QEMU exited before the VNC monitor was ready")
            try:
                connection.connect(str(monitor_path))
                break
            except (FileNotFoundError, ConnectionRefusedError):
                if time.monotonic() >= deadline:
                    raise RuntimeError("QEMU monitor did not become ready")
                time.sleep(0.1)
        qmp = Qmp(connection, process)
        if "QMP" not in qmp.receive():
            raise RuntimeError("Invalid greeting from QEMU monitor")
        qmp.execute("qmp_capabilities")
        status = qmp.execute("query-vnc")
        if not status.get("enabled"):
            raise RuntimeError("QEMU VNC display is not enabled")
        on_ready()
        if not status.get("clients"):
            while not qmp.viewer_connected:
                qmp.receive()
        print("VNC viewer connected. Starting the guest now.", flush=True)
        qmp.execute("cont")


def main():
    parser = argparse.ArgumentParser(
        description="Boot the installer ISO in a disposable UEFI VM"
    )
    parser.add_argument("--iso", type=Path)
    displays = parser.add_mutually_exclusive_group()
    displays.add_argument("--display", default="gtk", choices=["gtk", "sdl", "none"])
    displays.add_argument(
        "--vnc",
        type=int,
        nargs="?",
        const=1,
        metavar="DISPLAY",
        help="Headless VNC on localhost; display 1 is TCP 5901 (default)",
    )
    parser.add_argument("--memory", type=int, default=6144, help="Guest RAM in MiB")
    parser.add_argument("--cpus", type=int, default=2)
    parser.add_argument(
        "--disk-size",
        type=int,
        help="Fresh qcow2 size in GiB; derived from payload by default",
    )
    parser.add_argument(
        "--keep",
        action="store_true",
        help="Keep the disk, firmware variables and serial log",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print launch command without creating or booting a VM",
    )
    args = parser.parse_args()
    if args.vnc is not None and not 0 <= args.vnc <= 99:
        parser.error("--vnc display must be between 0 and 99")
    for tool in ["qemu-system-x86_64", "qemu-img"]:
        if not shutil.which(tool):
            parser.error(f"{tool} is required")
    images = sorted((HERE / "artifacts").glob("installer*.iso"))
    iso = args.iso or (images[0] if len(images) == 1 else None)
    if iso is None or not iso.is_file():
        parser.error("Build the ISO first, or provide --iso PATH")
    iso = iso.resolve()
    if "," in str(iso):
        parser.error("QEMU drive paths containing commas are not supported")
    code = next((path for path in CODE_CANDIDATES if path.is_file()), None)
    if code is None:
        parser.error("OVMF firmware is required")
    variables = code.with_name(code.name.replace("CODE", "VARS"))
    if not variables.is_file():
        parser.error(f"Matching OVMF variables not found: {variables}")
    metadata_path = HERE / ".work/payload.json"
    recommended = (
        json.loads(metadata_path.read_text())["recommendedDiskGiB"]
        if metadata_path.exists()
        else 8
    )
    disk_size = recommended if args.disk_size is None else args.disk_size
    if disk_size < recommended or args.memory < 1024 or args.cpus < 1:
        parser.error(f"Use at least {recommended} GiB disk, 1024 MiB RAM, and one CPU")
    if args.dry_run:
        print(
            shlex.join(
                command(
                    iso,
                    HERE / ".vm/NEW-DISPOSABLE-VM",
                    code,
                    args.memory,
                    args.cpus,
                    args.display,
                    args.vnc,
                )
            )
        )
        return
    state = HERE / ".vm"
    state.mkdir(exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="installer-", dir=state))
    try:
        shutil.copy2(variables, scratch / "vars.fd")
        os.chmod(scratch / "vars.fd", 0o600)
        subprocess.run(
            [
                "qemu-img",
                "create",
                "-f",
                "qcow2",
                str(scratch / "disk.qcow2"),
                f"{disk_size}G",
            ],
            check=True,
        )
        launch = command(
            iso, scratch, code, args.memory, args.cpus, args.display, args.vnc
        )
        print(
            f"ISO: {iso}\nDisposable disk: {scratch / 'disk.qcow2'}\nSerial log: {scratch / 'serial.log'}",
            flush=True,
        )
        print(
            "Disk-first UEFI boot: the blank disk falls through to the ISO; after Reboot the installed OS wins.",
            flush=True,
        )
        if args.vnc is not None:
            port = 5900 + args.vnc
        process = subprocess.Popen(launch)
        try:
            if args.vnc is not None:

                def show_vnc_instructions():
                    print(
                        f"Guest paused; VNC on localhost:{port} only. Tunnel from your local machine:\n"
                        f"  ssh -N -L {port}:127.0.0.1:{port} USER@HOST\n"
                        f"Connect to vnc://127.0.0.1:{port}; boot begins after the viewer connects.",
                        flush=True,
                    )

                wait_for_vnc_viewer(
                    process, scratch / "monitor.sock", show_vnc_instructions
                )
            status = process.wait()
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        if status:
            raise subprocess.CalledProcessError(status, launch)
    finally:
        if args.keep:
            print(f"Kept VM files: {scratch}")
        else:
            shutil.rmtree(scratch)


if __name__ == "__main__":
    with contextlib.suppress(KeyboardInterrupt):
        main()
