#!/usr/bin/env python3
"""Build test-only ACL-shaped Azure Linux images, never production ACL images."""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tarfile

HERE = Path(__file__).resolve().parent / "synthetic-acl"
ACL_UUIDS = {2: "7130c94a-213a-4e5a-8e26-6cce9662f132",
             3: "b736baf1-cdb4-4535-beba-ddaaa30ad7b7",
             4: "e03dd35c-7c2d-4a47-b3fe-27f15780a57c",
             5: "35bdf78b-c453-4661-98e6-f834f534ef5b"}
ACL_USR_TYPE = "5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6"


def run(command, **kwargs):
    return subprocess.run(command, check=True, text=True, **kwargs)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def provenance(path):
    return {"path": str(path), "sha256": sha256(path)}


def config(variant, phase, trident_nevra, installed_packages=None):
    # AZL's builder needs /boot/efi. normalize_disk relocates the mount,
    # not the EFI files, to ACL's /boot after UKI generation completes.
    partitions = [("esp", "EFI-SYSTEM", "esp", "512M"),
                  ("usr-a", "USR-A", "usr", "3G"),
                  ("hash-a", "HASH-A", "usr-verity", "128M"),
                  ("usr-b", "USR-B", "usr", "3G"),
                  ("hash-b", "HASH-B", "usr-verity", "128M"),
                  ("oem", "OEM", "linux-generic", "128M"),
                  ("root", "ROOT", "root", "4G")]
    packages = [trident_nevra, "device-mapper", "dracut",
                "efibootmgr", "iproute", "jq", "openssh-server", "systemd-boot",
                "systemd-networkd", "systemd-udev", "tar", "veritysetup",
                "kernel-mshv" if variant == "mshv" else "kernel"]
    # Azure Linux's official MSHV image uses shim 15.8: shim 16.1 is
    # incompatible with mshv-bootloader-lx (AZL work item 63139272).
    if variant == "mshv":
        packages.append("shim-15.8-6.azl3.x86_64")
    removals = ["grub2-efi-binary", "kernel", "shim"] if variant == "mshv" else ["grub2-efi-binary", "kernel-mshv"]
    if installed_packages is not None:
        removals = [name for name in removals if name in installed_packages]
    return {
        "storage": {
            "bootType": "efi",
            "disks": [{"partitionTableType": "gpt", "partitions": [
                {"id": i, "label": label, "type": kind, "size": size}
                for i, label, kind, size in partitions]}],
            "verity": [{"id": "usrverity", "name": "usr",
                        "dataDeviceId": "usr-a", "hashDeviceId": "hash-a",
                        "dataDeviceMountIdType": "part-label",
                        "hashDeviceMountIdType": "part-label",
                        "corruptionOption": "panic"}],
            "filesystems": [
                {"deviceId": "esp", "type": "fat32", "mountPoint": {
                    "idType": "uuid", "path": "/boot/efi", "options": "umask=0077"}},
                {"deviceId": "usrverity", "type": "ext4", "mountPoint": {
                    "path": "/usr", "options": "defaults,ro"}},
                {"deviceId": "oem", "type": "ext4", "mountPoint": {
                    "idType": "uuid", "path": "/oem"}},
                {"deviceId": "root", "type": "ext4", "mountPoint": {
                    "idType": "uuid", "path": "/"}}]},
        "os": {
            "bootloader": {"resetType": "hard-reset"},
            "hostname": "synthetic-acl-" + variant,
            "selinux": {"mode": "disabled"}, "uki": {"mode": "create"},
            "kernelCommandLine": {"extraCommandLine": [
                "console=ttyS0,115200", "console=tty0", "rd.hostonly=0",
                "mount.usr=/dev/mapper/usr", "mount.usrflags=ro"]},
            "packages": {"install": packages, "remove": removals},
            "users": [{"name": "root", "sshPublicKeyPaths": ["id_rsa.pub"]}],
            "services": {"enable": ["sshd", "systemd-networkd", "tridentd.socket"]},
            "additionalFiles": [{"source": "99-synthetic-acl.conf",
                                 "destination": "/etc/dracut.conf.d/99-synthetic-acl.conf"}]},
        "scripts": {"postCustomization": [
            {"path": "configure.sh", "arguments": [variant, phase]}]},
        "previewFeatures": ["uki"]}


def guestfish(image, commands, env, readonly=False):
    return run(["guestfish", "--ro" if readonly else "--rw", "-a", str(image)],
               input="run\n" + "\n".join(commands) + "\n", env=env,
               capture_output=True).stdout


def normalize_disk(image, work, variant, loader, payload, env):
    commands = [f"part-set-gpt-guid /dev/sda {n} {guid}" for n, guid in ACL_UUIDS.items()]
    commands += [f"part-set-gpt-type /dev/sda {n} {ACL_USR_TYPE}" for n in (2, 4)]
    commands += ["set-label /dev/sda1 EFI-SYSTEM", "set-label /dev/sda7 ROOT",
                 "mount /dev/sda7 /", "mount /dev/sda1 /boot/efi"]
    fstab = work / "fstab"
    fstab.write_text("LABEL=ROOT / ext4 defaults 0 1\n"
                     "LABEL=EFI-SYSTEM /boot vfat umask=0077 0 2\n"
                     "/dev/mapper/usr /usr ext4 defaults,ro 0 0\n"
                     "PARTLABEL=OEM /oem ext4 defaults 0 2\n")
    commands.append(f"upload {shlex.quote(str(fstab))} /etc/fstab")
    if variant == "mshv":
        # shim -> grubx64 (ETL) -> HvLoader -> second (systemd-boot) -> UKI.
        commands += ["cp /boot/efi/EFI/BOOT/grubx64.efi /boot/efi/EFI/BOOT/second.efi",
                     f"upload {shlex.quote(str(loader))} /boot/efi/EFI/BOOT/grubx64.efi"]
        for name in ("HvLoader.efi", "lxhvloader.dll", "Windows"):
            source = payload / name
            if source.is_dir():
                commands.append(f"copy-in {shlex.quote(str(source))} /boot/efi/EFI/BOOT")
            elif source.is_file():
                commands.append(f"upload {shlex.quote(str(source))} /boot/efi/EFI/BOOT/{name}")
    # Remove the builder mountpoint from shared ROOT, never from the mounted ESP.
    guestfish(image, commands + ["umount /boot/efi", "rmdir /boot/efi", "umount-all"], env)


def split_verity_cmdline(cmdline):
    tokens = cmdline.split()
    hashes = {token.split("=", 1)[1] for token in tokens if token.startswith("usrhash=")}
    if len(hashes) != 1:
        raise ValueError("Expected one verity root hash in the ImageCustomizer command line")
    root_hash = hashes.pop()
    if len(root_hash) not in (64, 128) or any(c not in "0123456789abcdefABCDEF" for c in root_hash):
        raise ValueError("Invalid verity root hash in the ImageCustomizer command line")
    slot_keys = {"usrhash", "acl.slot", "systemd.verity_usr_data",
                 "systemd.verity_usr_hash", "systemd.verity_usr_options"}
    common = ["root=LABEL=ROOT" if token.startswith("root=") else token
              for token in tokens if token.split("=", 1)[0] not in slot_keys]
    addons = {}
    for slot, data, hashes in (("a", 2, 3), ("b", 4, 5)):
        addons[slot] = (f"systemd.verity_usr_data=PARTUUID={ACL_UUIDS[data]} "
                        f"systemd.verity_usr_hash=PARTUUID={ACL_UUIDS[hashes]} "
                        f"systemd.verity_usr_options=panic-on-corruption usrhash={root_hash} acl.slot={slot}")
    return " ".join(common), addons


def pe_cmdline(path, env):
    sections = run(["objdump", "-h", str(path)], capture_output=True, env=env).stdout
    if not any(line.split()[1:2] == [".cmdline"] for line in sections.splitlines()):
        return ""
    destination = path.with_suffix(path.suffix + ".cmdline")
    run(["objcopy", "--dump-section", f".cmdline={destination}", str(path)], env=env)
    return destination.read_bytes().rstrip(b"\0").decode().strip()


def install_verity_addons(image, work, image_customizer, env):
    work.mkdir()
    # Use the image's stub and OS identity so systemd accepts the resulting addons.
    guestfish(image, ["mount-ro /dev/sda2 /",
              f"download /lib/systemd/boot/efi/linuxx64.efi.stub {shlex.quote(str(work / 'stub.efi'))}",
              f"download /lib/os-release {shlex.quote(str(work / 'os-release'))}",
              "umount-all", "mount-ro /dev/sda1 /",
              f"copy-out /EFI/Linux {shlex.quote(str(work))}"], env, readonly=True)
    ukis = sorted((work / "Linux").glob("*.efi"))
    if len(ukis) != 1:
        raise ValueError("Expected exactly one synthetic image UKI")
    uki = ukis[0]
    active_dir = uki.with_name(uki.name + ".extra.d")
    providers = [(path, pe_cmdline(path, env))
                 for path in [uki, *sorted(active_dir.glob("*.addon.efi"))]]
    providers = [(path, cmdline) for path, cmdline in providers if cmdline]
    if len(providers) != 1:
        raise ValueError("Expected exactly one ImageCustomizer command-line section")
    provider, cmdline = providers[0]
    common, addons = split_verity_cmdline(cmdline)
    cmdline_file = work / "common.cmdline"
    cmdline_file.write_bytes(common.encode() + b"\0")
    rewritten = provider.with_suffix(".rewritten.efi")
    run(["objcopy", "--update-section", f".cmdline={cmdline_file}", str(provider), str(rewritten)], env=env)
    rewritten.replace(provider)
    if pe_cmdline(provider, env) != common:
        raise ValueError("Rewritten UKI command line does not match the slot-independent policy")
    commands = ["mount /dev/sda1 /",
                f"upload {shlex.quote(str(provider))} /EFI/{provider.relative_to(work).as_posix()}",
                "mkdir-p /acl/uki-addons",
                f"mkdir-p /EFI/Linux/{uki.name}.extra.d"]
    for slot, cmdline in addons.items():
        name = f"slot-{slot}.addon.efi"
        (work / f"slot-{slot}.cmdline").write_text(cmdline + "\n")
        run(["docker", "run", "--rm", "--entrypoint", "/usr/bin/ukify",
             "-e", "TMPDIR=/work", "-v", f"{work}:/work", image_customizer,
             "build", "--stub=/work/stub.efi", "--os-release=@/work/os-release",
             f"--cmdline=@/work/slot-{slot}.cmdline", f"--output=/work/{name}"], env=env)
        addon = work / name
        if pe_cmdline(addon, env) != cmdline:
            raise ValueError(f"Slot {slot} addon does not contain its requested verity command line")
        commands.append(f"upload {shlex.quote(str(addon))} /acl/uki-addons/{name}")
        if slot == "a":
            commands.append(f"upload {shlex.quote(str(addon))} /EFI/Linux/{uki.name}.extra.d/{name}")
    guestfish(image, commands + ["umount-all"], env)


def publish_metadata(cosi, output, variant):
    # COSI 1.3 adds the MSHV discriminator; partition bytes/hashes stay intact.
    replacement = cosi.with_suffix(".rewritten.cosi")
    with tarfile.open(cosi, "r:") as source, tarfile.open(replacement, "w:") as target:
        for member in source:
            stream = source.extractfile(member) if member.isfile() else None
            if member.name == "metadata.json":
                metadata = json.load(stream)
                usr = next((i for i in metadata["images"] if i["mountPoint"] == "/usr"), None)
                if not usr or not (usr.get("verity") or {}).get("roothash"):
                    raise ValueError("COSI must contain a verity-protected /usr")
                boot = metadata.get("bootloader", {})
                allowed = {"systemd-boot", "efitreeloader-mshv-systemd-boot"} if variant == "mshv" else {"systemd-boot"}
                if boot.get("type") not in allowed or not boot.get("systemdBoot", {}).get("entries"):
                    raise ValueError("COSI must contain systemd-boot UKI metadata")
                if variant == "mshv":
                    metadata["version"] = "1.3"
                    boot["type"] = "efitreeloader-mshv-systemd-boot"
                data = (json.dumps(metadata, indent=2) + "\n").encode()
                (output / "metadata.json").write_bytes(data)
                member.size = len(data)
                stream = io.BytesIO(data)
            target.addfile(member, stream)
    if not (output / "metadata.json").exists():
        raise ValueError("COSI has no metadata.json")
    replacement.replace(cosi)


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--variant", choices=("standard", "mshv"), required=True)
    for name in ("base-image", "rpm-dir", "ssh-public-key", "ssh-private-key", "output-dir"):
        result.add_argument("--" + name, type=Path, required=True)
    result.add_argument("--image-customizer", required=True, help="ImageCustomizer Docker reference")
    result.add_argument("--efitreeloader", type=Path)
    result.add_argument("--mshv-payload", type=Path, help="HvLoader.efi, lxhvloader.dll and Windows/ component tree")
    result.add_argument("--loader-provenance", help="Source revision and load_entries.json identity")
    return result


def main(argv=None):
    args = parser().parse_args(argv)
    if os.geteuid() != 0:
        raise ValueError("Run with sudo: ImageCustomizer creates root-owned disk images")
    for tool in ("docker", "guestfish", "rpm", "ssh-keygen", "objcopy", "objdump"):
        if not shutil.which(tool):
            raise ValueError(f"Missing required tool: {tool}")
    for name in ("base_image", "ssh_public_key", "ssh_private_key", "rpm_dir"):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    if args.variant == "mshv":
        if not all((args.efitreeloader, args.mshv_payload, args.loader_provenance)):
            raise ValueError("mshv requires --efitreeloader, --mshv-payload and --loader-provenance")
        args.efitreeloader = args.efitreeloader.resolve(strict=True)
        args.mshv_payload = args.mshv_payload.resolve(strict=True)
        for name in ("HvLoader.efi", "lxhvloader.dll"):
            if not (args.mshv_payload / name).is_file():
                raise ValueError(f"Missing MSHV payload: {name}")
        windows = args.mshv_payload / "Windows"
        if not windows.is_dir() or not any(p.is_file() for p in windows.rglob("*")):
            raise ValueError("Missing nonempty MSHV Windows component tree")
        for path in args.mshv_payload.rglob("*"):
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                raise ValueError(f"Payload must contain only regular files/directories: {path}")
    elif any((args.efitreeloader, args.mshv_payload, args.loader_provenance)):
        raise ValueError("standard must not contain MSHV loader inputs")
    public = run(["ssh-keygen", "-y", "-P", "", "-f", str(args.ssh_private_key)], capture_output=True).stdout.split()
    if public[:2] != args.ssh_public_key.read_text().split()[:2]:
        raise ValueError("SSH public/private keys do not match")
    rpms = sorted(args.rpm_dir.glob("*.rpm"))
    trident = []
    for rpm in rpms:
        fields = run(["rpm", "-qp", "--qf", "%{NAME} %{VERSION} %{RELEASE} %{ARCH}", str(rpm)], capture_output=True).stdout.split()
        if fields[0] == "trident" and fields[3] == "x86_64":
            trident.append((rpm, fields))
    if len(trident) != 1:
        raise ValueError("RPM directory must contain exactly one x86_64 trident RPM")
    _, (_, version, release, arch) = trident[0]
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise ValueError("Output directory must be empty (failed builds retain diagnostics)")
    work = output / ".work"
    work.mkdir()
    env = os.environ.copy()
    for key in ("TMPDIR", "LIBGUESTFS_TMPDIR", "LIBGUESTFS_CACHEDIR"):
        directory = work / key.lower()
        directory.mkdir()
        env[key] = str(directory)
    os_release = run(["guestfish", "--ro", "-a", str(args.base_image), "-i", "cat", "/etc/os-release"],
                  capture_output=True, env=env).stdout
    os_release = dict(line.split("=", 1) for line in os_release.splitlines() if "=" in line and not line.startswith("#"))
    os_release = {key: value.strip('\"') for key, value in os_release.items()}
    if os_release.get("ID") != "azurelinux" or os_release.get("VERSION_ID") != "3.0" or os_release.get("VARIANT_ID") == "azurecontainerlinux":
        raise ValueError("Base image must be plain x86-64 Azure Linux 3.0, not an ACL/nightly image")
    # Older libguestfs inspection cannot read Azure Linux's SQLite RPM database.
    inventory = run(["guestfish", "--ro", "-a", str(args.base_image), "-i", "command",
                     "rpm -qa --qf %{NAME}|"], capture_output=True, env=env).stdout
    installed_packages = set(filter(None, inventory.strip().split("|")))
    if not installed_packages:
        raise ValueError("Could not read the base image's installed RPM inventory")
    with args.ssh_private_key.open("rb") as source, os.fdopen(os.open(
            output / "id_rsa", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as target:
        shutil.copyfileobj(source, target)
    shutil.copyfile(args.ssh_public_key, work / "id_rsa.pub")
    local_rpms = work / "rpms"
    local_rpms.mkdir()
    for rpm in rpms:
        shutil.copy2(rpm, local_rpms / rpm.name)
    for name in ("configure.sh", "99-synthetic-acl.conf"):
        shutil.copy2(HERE / name, work / name)
    if args.variant == "mshv" and (args.mshv_payload / "licenses").is_dir():
        shutil.copytree(args.mshv_payload / "licenses", work / "mshv-licenses")
    image_id = run(["docker", "image", "inspect", "--format", "{{.Id}}", args.image_customizer], capture_output=True).stdout.strip()
    run(["docker", "run", "--rm", "--entrypoint", "/usr/bin/ukify", image_id, "--version"], env=env)
    for phase in ("base", "update"):
        configuration = config(args.variant, phase, f"trident-{version}-{release}.{arch}", installed_packages)
        if (work / "mshv-licenses").is_dir():
            for license_file in sorted((work / "mshv-licenses").rglob("*")):
                if license_file.is_file():
                    relative = license_file.relative_to(work / "mshv-licenses")
                    configuration["os"]["additionalFiles"].append({
                        "source": str(Path("mshv-licenses") / relative), "destination": str(Path("/usr/share/licenses") / relative)})
        (work / f"{phase}.yaml").write_text(json.dumps(configuration, indent=2) + "\n")
        image = work / f"{phase}.qcow2"
        run(["docker", "run", "--rm", "--privileged", "-v", f"{work}:/work",
             "-v", f"{args.base_image}:/input/base.img:ro",
             "-v", "/dev:/dev", image_id, "customize", "--config-file", f"/work/{phase}.yaml",
             "--build-dir", f"/work/build-{phase}", "--image-file", "/input/base.img",
             "--output-image-format", "qcow2", "--output-image-file", f"/work/{phase}.qcow2",
             "--rpm-source", "/work/rpms"], env=env)
        normalize_disk(image, work, args.variant, args.efitreeloader, args.mshv_payload, env)
        install_verity_addons(image, work / f"uki-{phase}", image_id, env)
    shutil.move(work / "base.qcow2", output / "base.qcow2")
    run(["docker", "run", "--rm", "--privileged", "-v", f"{work}:/work",
         "-v", f"{output}:/output", "-v", "/dev:/dev", image_id, "convert",
         "--build-dir", "/work/build-cosi", "--image-file", "/work/update.qcow2",
         "--output-image-format", "cosi", "--output-image-file", "/output/update.cosi"], env=env)
    publish_metadata(output / "update.cosi", output, args.variant)
    binary = work / "trident"
    guestfish(output / "base.qcow2", ["mount-ro /dev/sda2 /",
              f"download /bin/trident {shlex.quote(str(binary))}"], env, readonly=True)
    update_binary = work / "trident-update"
    guestfish(work / "update.qcow2", ["mount-ro /dev/sda2 /",
              f"download /bin/trident {shlex.quote(str(update_binary))}"], env, readonly=True)
    if sha256(binary) != sha256(update_binary):
        raise ValueError("Base and update must contain identical pipeline Trident binaries")
    manifest = {
        "variant": args.variant, "synthetic": True,
        "expectedTridentVersion": version, "expectedTridentRelease": release,
        "expectedTridentSha256": sha256(binary),
        "osRelease": {"ID": "azurelinux", "VARIANT_ID": "azurecontainerlinux", "VERSION_ID": "3.0"},
        "network": {"dhcp": True, "address": None},
        "inputs": {"baseImage": provenance(args.base_image), "rpms": [provenance(p) for p in rpms],
                   "imageCustomizer": {"reference": args.image_customizer, "imageId": image_id}},
        "metadataSha384": hashlib.sha384((output / "metadata.json").read_bytes()).hexdigest(),
        "outputs": {name: {"sha256": sha256(output / name)}
                    for name in ("base.qcow2", "update.cosi", "metadata.json")}}
    if args.variant == "mshv":
        manifest["inputs"]["efitreeloader"] = {**provenance(args.efitreeloader), "provenance": args.loader_provenance}
        manifest["inputs"]["mshvPayload"] = [provenance(p) for p in sorted(args.mshv_payload.rglob("*")) if p.is_file()]
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
