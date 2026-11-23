#!/usr/bin/env python3

"""Validation-only, unreviewed measurements for producer 1266405."""

import argparse
import hashlib
import json
import os
import re
import selectors
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
import zipfile

from pathlib import Path, PurePosixPath

import download_functional_artifact as ado
import prepare_vm_e2e_artifacts as prepare

BUILD = 1266405
REVISION = "18dd6cd99d50a4268afd936acb1ee4d2b64881ea"
REF = "refs/heads/user/britel/cloud-vm-e2e-install"
MIC = "3e7535e520cfed39561c1040f3ca2ee1ba96dd16bfd89aa643c74fde87c9e614"
BASE = {
    "organization": "mariner-org",
    "projectId": "36d030d6-1d99-4ebd-878b-09af1f4f722f",
    "feed": "AzureLinuxArtifacts",
    "package": "baremetal_vhdx-3.0-stable",
    "version": "3.0.20261116",
    "selectedFile": "core-3.0.20261116.vhdx",
}
ARTIFACT_IDS = dict(
    zip(
        ("go-tools", "trident-binaries", *prepare.INSTALLERS, *prepare.IMAGES),
        (5617669, 5617718, 5617787, 5617785, 5617779, 5617776, 5617792),
    )
)
# Fixed task logs only. No job, environment, Cargo authentication or user selector.
INPUT_CITES = {
    "trident-installer": ((280, 39, 47), (327, 30, 44), (327, 68, 69)),
    "trident-split-installer": ((275, 39, 47), (323, 30, 44), (323, 68, 69)),
    "trident-testimage": ((264, 39, 47), (301, 32, 72), (301, 121, 124)),
    "trident-verity-testimage": ((236, 39, 47), (299, 32, 72), (299, 121, 124)),
    "trident-usrverity-testimage": ((278, 39, 47), (325, 38, 78), (325, 127, 130)),
}
SOURCE_LOGS = {
    "go-tools": (58,),
    "trident-binaries": (10, 83, 114),
    "trident-installer": (166, 180, 293),
    "trident-split-installer": (149, 175, 289),
    "trident-testimage": (141, 167, 276),
    "trident-verity-testimage": (132, 137, 243),
    "trident-usrverity-testimage": (159, 179, 291),
}
CRITICAL = {
    "trident": (
        "/usr/bin/trident",
        "/usr/lib/systemd/system/tridentd.service",
        "/usr/lib/systemd/system/tridentd.socket",
    ),
    "trident-service": ("/usr/lib/systemd/system/trident.service",),
    "trident-provisioning": ("/usr/lib/systemd/system/trident-network.service",),
    "trident-install-service": ("/usr/lib/systemd/system/trident-install.service",),
    "trident-selinux": (
        "/usr/share/selinux/packages/targeted/trident.pp.bz2",
        "/usr/share/selinux/devel/include/distributed/trident.if",
    ),
    "trident-acl": (
        "/usr/bin/trident-acl-agent",
        "/usr/lib/systemd/system/trident-acl-agent.service",
    ),
    "trident-installer": (
        "/usr/bin/trident-installer",
        "/usr/lib/systemd/system/trident-installer.service",
    ),
}
QF = "%{NAME}\\t%{EPOCHNUM}\\t%{VERSION}\\t%{RELEASE}\\t%{ARCH}\\n"
CONFIG = {
    "organization": "mariner-org",
    "project": prepare.PROJECT_ID,
    "definitionId": 5073,
    "sourceBranch": REF,
    "buildStatus": "completed",
    "buildResult": "succeeded",
}
COMMANDS = ("az", "rpm", "rpm2cpio", "cpio", "guestfish", "zstd")
VM_RUNNER = prepare.reader()
MAX_RAW_SIZE = 16 * 1024**3


class Failure(RuntimeError):
    pass


class Cancelled(BaseException):
    pass


def require(condition, fact):
    if not condition:
        raise Failure(fact)


def measure(path):
    require(path.is_file() and not path.is_symlink(), "regular-file-required")
    return {"size": path.stat().st_size, "sha256": ado.hash_file(path)}


def command(
    argv,
    *,
    data=None,
    input_file=None,
    output=None,
    timeout=600,
    failure_fact="inspection-command-failed",
):
    # Never inherit tool output. Even failures can contain auth URLs or payloads.
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        process = subprocess.Popen(
            argv,
            stdin=(
                subprocess.PIPE
                if data is not None
                else input_file if input_file is not None else subprocess.DEVNULL
            ),
            stdout=output if output is not None else stdout,
            stderr=stderr,
            start_new_session=True,
        )
        try:
            process.communicate(data.encode() if data is not None else None, timeout)
        except BaseException as primary:
            try:
                VM_RUNNER.reap(process, 10, group=True)
            finally:
                raise primary
        VM_RUNNER.signal_group(process, signal.SIGKILL)
        require(process.returncode == 0, failure_fact)
        stdout.seek(0)
        result = stdout.read(1024 * 1024 + 1)
        require(len(result) <= 1024 * 1024, "inspection-output-limit")
        return result.decode("utf-8", errors="strict")


def extract_selected(archive, target, filenames):
    # Reuse the reviewed whole-ZIP path/type guards and B's unique selection.
    files = {}
    with zipfile.ZipFile(archive) as source:
        for filename in filenames:
            entry = ado.find_image_entry(source, filename)
            digest = hashlib.sha256()
            with source.open(entry) as stream:
                while chunk := stream.read(ado.CHUNK_SIZE):
                    digest.update(chunk)
            files[filename] = {"size": entry.file_size, "sha256": digest.hexdigest()}
    prepare.extract(archive, target, files)
    return files


def rpm_rows(text):
    result = {}
    for line in text.splitlines():
        fields = line.split("\t")
        require(len(fields) == 5, "rpm-query-layout")
        name, epoch, version, release, arch = fields
        if not (name == "trident" or name.startswith("trident-")):
            continue
        for value in fields:
            require(
                re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+~-]*", value), "rpm-identity"
            )
        require(epoch.isdecimal() and name not in result, "ambiguous-rpm-identity")
        result[name] = f"{name}-{epoch}:{version}-{release}.{arch}"
    return result


def rpm_payload(path, name):
    require(
        command(["rpm", "--noplugins", "-qp", "--qf", "%{FILEDIGESTALGO}", str(path)])
        == "8",
        "rpm-payload-not-sha256",
    )
    text = command(
        [
            "rpm",
            "--noplugins",
            "-qp",
            "--qf",
            "[%{FILENAMES}\\t%{FILEDIGESTS}\\t%{FILEMODES}\\n]",
            str(path),
        ]
    )
    payload = {}
    for line in text.splitlines():
        fields = line.split("\t")
        require(len(fields) == 3, "rpm-file-query-layout")
        filename, digest, mode = fields
        selected = filename in CRITICAL.get(name, ())
        if name == "trident-static-pcrlock-files":
            selected = bool(
                re.fullmatch(r"/var/lib/pcrlock.d/[A-Za-z0-9._/-]+", filename)
                and ".." not in PurePosixPath(filename).parts
            )
        if selected and stat.S_ISREG(int(mode)):
            require(
                prepare.sha256(digest) and filename not in payload, "rpm-file-digest"
            )
            payload[filename] = digest
    require(
        set(CRITICAL.get(name, ())) <= set(payload) and payload,
        "missing-critical-rpm-payload",
    )
    with tempfile.TemporaryDirectory(
        dir=path.parent, prefix="rpm-payload-"
    ) as directory:
        archive = Path(directory) / "payload.cpio"
        with archive.open("wb") as target:
            command(["rpm2cpio", str(path)], output=target)
        for filename, expected in payload.items():
            extracted = Path(directory) / "critical"
            with archive.open("rb") as source, extracted.open("wb") as target:
                command(
                    [
                        "cpio",
                        "--quiet",
                        "--extract",
                        "--to-stdout",
                        "--no-absolute-filenames",
                        "." + filename,
                    ],
                    input_file=source,
                    output=target,
                )
            actual = measure(extracted)
            require(
                actual["size"] > 0 and actual["sha256"] == expected,
                "source-rpm-payload-digest-mismatch",
            )
    return payload


def source_rpms(directory):
    packages = {}
    for path in sorted(directory.glob("*.rpm")):
        rows = rpm_rows(command(["rpm", "--noplugins", "-qp", "--qf", QF, str(path)]))
        require(len(rows) == 1, "source-rpm-identity")
        name, nevra = next(iter(rows.items()))
        require(name not in packages, "ambiguous-source-rpm")
        packages[name] = {
            "name": name,
            "nevra": nevra,
            **measure(path),
            "payload": (
                rpm_payload(path, name)
                if name in CRITICAL or name == "trident-static-pcrlock-files"
                else {}
            ),
        }
    require({"trident", "trident-service"} <= set(packages), "missing-source-rpms")
    return packages


def guest(drives, mounts, commands):
    argv = ["guestfish", "--ro"]
    # No inspection auto-mount, guest commands, host mounts or image executables.
    for path, fmt in drives:
        argv.extend(["--format=" + fmt, "-a", str(path)])
    script = ["run"]
    script.extend(f"mount-ro {device} {mount}" for device, mount in mounts)
    script.extend(commands)
    return command(argv, data="\n".join(script) + "\n")


def decompress_cosi(compressed, raw, expected_size, timeout=600):
    require(
        type(expected_size) is int and 0 < expected_size <= MAX_RAW_SIZE,
        "cosi-image-layout",
    )
    argv = ["zstd", "-d", "-c", str(compressed)]
    with raw.open("xb") as target:
        try:
            with tempfile.TemporaryFile() as stderr:
                process = subprocess.Popen(
                    argv,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=stderr,
                    start_new_session=True,
                )
                try:
                    deadline = time.monotonic() + timeout
                    written = 0
                    with selectors.DefaultSelector() as selector:
                        selector.register(process.stdout, selectors.EVENT_READ)
                        while True:
                            remaining = deadline - time.monotonic()
                            if remaining <= 0 or not selector.select(remaining):
                                raise subprocess.TimeoutExpired(argv, timeout)
                            chunk = os.read(
                                process.stdout.fileno(),
                                min(ado.CHUNK_SIZE, expected_size - written + 1),
                            )
                            if not chunk:
                                break
                            require(
                                len(chunk) <= expected_size - written,
                                "cosi-raw-size-overflow",
                            )
                            target.write(chunk)
                            written += len(chunk)
                    status = process.wait(timeout=max(0, deadline - time.monotonic()))
                    VM_RUNNER.signal_group(process, signal.SIGKILL)
                    require(status == 0, "inspection-command-failed")
                    require(written == expected_size, "cosi-raw-size")
                except BaseException as primary:
                    try:
                        VM_RUNNER.reap(process, 10, group=True)
                    finally:
                        raise primary
                finally:
                    process.stdout.close()
        except BaseException as primary:
            try:
                raw.unlink(missing_ok=True)
            finally:
                raise primary


def cosi_layout(path, work):
    with tarfile.open(path, "r:") as archive:
        members = archive.getmembers()
        names = [entry.name for entry in members]
        require(len(names) == len(set(names)) and len(names) <= 128, "cosi-members")
        for entry in members:
            name = PurePosixPath(entry.name)
            require(
                not name.is_absolute()
                and ".." not in name.parts
                and "\\" not in entry.name
                and ":" not in entry.name
                and (entry.isfile() or entry.isdir()),
                "cosi-unsafe-member",
            )
        metadata_entry = archive.getmember("metadata.json")
        require(metadata_entry.size <= 4 * 1024 * 1024, "cosi-metadata-limit")
        metadata = json.load(archive.extractfile(metadata_entry))
        require(metadata.get("osArch") in ("x86_64", "amd64"), "cosi-architecture")
        filesystems = metadata["images"]
        drives, mounts, seen = [], [], set()
        for image in sorted(filesystems, key=lambda row: len(row["mountPoint"])):
            mount = image["mountPoint"]
            require(mount not in seen, "cosi-duplicate-mount")
            seen.add(mount)
            require(mount in ("/", "/usr", "/boot", "/boot/efi"), "cosi-unknown-mount")
            if mount not in ("/", "/usr"):
                continue
            require(image["fsType"] == "ext4", "cosi-required-ext4")
            info = image["image"]
            require(
                re.fullmatch(r"images/[A-Za-z0-9._/-]+\.raw\.zst", info["path"])
                and 0 < info["uncompressedSize"] <= MAX_RAW_SIZE,
                "cosi-image-layout",
            )
            member = archive.getmember(info["path"])
            require(
                member.isfile() and member.size == info["compressedSize"], "cosi-size"
            )
            compressed = work / f"part-{len(drives)}.zst"
            with archive.extractfile(member) as source, compressed.open("wb") as target:
                shutil.copyfileobj(source, target)
            digest = hashlib.sha384()
            with compressed.open("rb") as source:
                while chunk := source.read(ado.CHUNK_SIZE):
                    digest.update(chunk)
            require(digest.hexdigest() == info["sha384"], "cosi-member-sha384")
            raw = compressed.with_suffix(".raw")
            decompress_cosi(compressed, raw, info["uncompressedSize"])
            require(raw.stat().st_size == info["uncompressedSize"], "cosi-raw-size")
            drives.append((raw, "raw"))
            mounts.append((f"/dev/sd{chr(96 + len(drives))}", mount))
        require("/" in seen and mounts and mounts[0][1] == "/", "cosi-root-missing")
        # osPackages is informational only, never substituted for RPMDB/payload proof.
        return drives, mounts


def inspect_image(path, work, packages):
    if path.suffix == ".iso":
        rootfs = work / "rootfs.img"
        drives, mounts = [(path, "raw")], [("/dev/sda", "/")]
        exists = guest(drives, mounts, ["is-file /liveos/rootfs.img"]).strip()
        require(exists == "true", "iso-missing-liveos-rootfs-img-rpmdb-layout")
        guest(
            drives, mounts, ["download /liveos/rootfs.img " + shlex.quote(str(rootfs))]
        )
        drives, mounts = [(rootfs, "raw")], [("/dev/sda", "/")]
    else:
        drives, mounts = cosi_layout(path, work)
    dbs = ("/usr/lib/sysimage/rpm/rpmdb.sqlite", "/var/lib/rpm/rpmdb.sqlite")
    found = guest(drives, mounts, ["is-file " + db for db in dbs]).splitlines()
    require(len(found) == 2 and "true" in found, "image-sqlite-rpmdb-missing")
    if found == ["true", "true"]:
        hashes = guest(
            drives, mounts, ["checksum sha256 " + db for db in dbs]
        ).splitlines()
        require(
            len(hashes) == 2 and prepare.sha256(hashes[0]) and hashes[0] == hashes[1],
            "image-ambiguous-rpmdb",
        )
    db = dbs[found.index("true")]
    require(
        guest(drives, mounts, ["exists " + db + "-wal"]).strip() == "false",
        "image-rpmdb-uncheckpointed-wal",
    )
    local_db = work / "rpmdb"
    local_db.mkdir()
    guest(
        drives,
        mounts,
        ["download " + db + " " + shlex.quote(str(local_db / "rpmdb.sqlite"))],
    )
    installed = rpm_rows(
        command(["rpm", "--noplugins", "--dbpath", str(local_db), "-qa", "--qf", QF])
    )
    required = {"trident"} if path.suffix == ".iso" else {"trident", "trident-service"}
    require(required <= set(installed), "image-required-package-missing")
    require(set(installed) <= set(packages), "image-unqualified-trident-package")
    checks, identities = [], []
    for name, nevra in sorted(installed.items()):
        package = packages[name]
        require(nevra == package["nevra"], "image-rpm-nevra-mismatch")
        require(package["payload"], "image-critical-package-layout-unknown")
        identities.append({key: package[key] for key in ("name", "nevra", "sha256")})
        checks.extend(package["payload"].items())
    hashes = guest(
        drives,
        mounts,
        ["checksum sha256 " + shlex.quote(filename) for filename, _ in checks],
    ).splitlines()
    require(len(hashes) == len(checks), "image-payload-query-layout")
    payload = {}
    for (filename, expected), actual in zip(checks, hashes):
        require(actual == expected, "image-critical-payload-mismatch")
        payload[filename] = actual
    return {
        "installedPackages": identities,
        "payloadSha256": payload,
        "basis": "rpmdb-and-read-only-payload",
    }


def collect_logs(token, records, stages, metadata):
    by_id = {row["id"]: row for row in records}
    selected = {name: set(ids) for name, ids in SOURCE_LOGS.items()}
    for name, citations in INPUT_CITES.items():
        selected[name].update(row[0] for row in citations)
    result = {}
    for name, log_ids in selected.items():
        for log_id in sorted(log_ids):
            tasks = [r for r in records if (r.get("log") or {}).get("id") == log_id]
            require(len(tasks) == 1 and tasks[0]["type"] == "Task", "log-task-identity")
            row, seen = tasks[0], set()
            while row["id"] != stages[prepare.STAGES[name]]:
                require(
                    row["id"] not in seen
                    and row.get("state") == "completed"
                    and row.get("result") == "succeeded",
                    "log-task-ancestry",
                )
                seen.add(row["id"])
                row = by_id[row["parentId"]]
            matches = [r for r in metadata if r["id"] == log_id]
            require(len(matches) == 1, "log-metadata-identity")
            request = urllib.request.Request(
                ado.build_api_url(
                    CONFIG, f"build/builds/{BUILD}/logs/{log_id}?api-version=7.1"
                ),
                headers={"Accept": "text/plain", "Authorization": "Bearer " + token},
            )
            opener = urllib.request.build_opener(ado.ArtifactRedirectHandler())
            with opener.open(request, timeout=120) as response:
                require(
                    response.headers.get_content_type() == "text/plain",
                    "log-body-content-type",
                )
                body = response.read(64 * 1024 * 1024 + 1)
            require(len(body) <= 64 * 1024 * 1024, "log-body-limit")
            count = len(body.splitlines())
            require(count == matches[0]["lineCount"], "log-line-count-mismatch")
            for cited_id, start, end in INPUT_CITES.get(name, ()):
                if cited_id == log_id:
                    require(1 <= start <= end <= count, "input-citation-line-range")
            result[str(log_id)] = {
                "sha256": hashlib.sha256(body).hexdigest(),
                "lineCount": count,
            }
    return result


def inspect(work, facts):
    args = argparse.Namespace(
        build_id=BUILD,
        expected_definition_id=5073,
        expected_revision=REVISION,
        expected_ref=REF,
    )
    token = command(
        [
            "az",
            "account",
            "get-access-token",
            "--resource",
            ado.ADO_RESOURCE,
            "--query",
            "accessToken",
            "-o",
            "tsv",
        ],
        timeout=120,
    ).strip()
    require(token and "\n" not in token, "ado-token-unavailable")
    build, records, stages, logs = prepare.verify_server(
        CONFIG, prepare.producer(args), token
    )
    require(build["definition"].get("revision") == 7, "producer-definition-revision")
    facts["logs"] = collect_logs(token, records, stages, logs)
    listing = ado.request_json(
        ado.build_api_url(CONFIG, f"build/builds/{BUILD}/artifacts?api-version=7.1"),
        token,
    )["value"]
    files = {}
    facts["artifacts"] = {}
    for name, artifact_id in ARTIFACT_IDS.items():
        matches = [r for r in listing if r.get("name") == name]
        require(
            len(matches) == 1 and matches[0].get("id") == artifact_id,
            "artifact-id-mismatch",
        )
        artifact = matches[0]
        url = ado.verify_artifact(
            {"artifactName": name, "artifactType": "PipelineArtifact"}, artifact
        )
        archive = work / (name + ".zip")
        ado.download_file(url, token, archive)
        if name == "go-tools":
            filenames = prepare.TOOLS
        elif name == "trident-binaries":
            with zipfile.ZipFile(archive) as source:
                filenames = [
                    PurePosixPath(r.filename).name
                    for r in source.infolist()
                    if re.fullmatch(
                        r"trident[A-Za-z0-9._+-]*\.rpm", PurePosixPath(r.filename).name
                    )
                    and ".src.rpm" not in r.filename
                    and ".azl3." in r.filename
                ]
            require(filenames and len(filenames) <= 32, "candidate-rpm-zip-layout")
        elif name in prepare.INSTALLERS:
            filenames = [name + ".iso"]
        else:
            filenames = [f"{name}_{index}.cosi" for index in range(2)]
        files[name] = extract_selected(archive, work / name, filenames)
        facts["artifacts"][name] = {
            "id": artifact_id,
            "advertisedSize": ado.artifact_size(artifact),
            "archive": measure(archive),
            "files": files[name],
        }
        archive.unlink()
    packages = source_rpms(work / "trident-binaries")
    facts["rpms"] = packages
    base = work / "base"
    base.mkdir()
    command(
        [
            "az",
            "artifacts",
            "universal",
            "download",
            "--organization",
            "https://dev.azure.com/mariner-org",
            "--project",
            BASE["projectId"],
            "--scope",
            "project",
            "--feed",
            BASE["feed"],
            "--name",
            BASE["package"],
            "--version",
            BASE["version"],
            "--file-filter",
            BASE["selectedFile"],
            "--path",
            str(base),
            "--only-show-errors",
        ],
        timeout=1200,
        failure_fact="exact-base-universal-download-failed",
    )
    facts["baseVhdx"] = {**BASE, **measure(base / BASE["selectedFile"])}
    for name in (*prepare.INSTALLERS, *prepare.IMAGES):
        for filename in files[name]:
            facts["activeInspection"] = {"artifact": name, "file": filename}
            with tempfile.TemporaryDirectory(dir=work, prefix="inspect-") as directory:
                proof = inspect_image(work / name / filename, Path(directory), packages)
                facts["artifacts"][name]["files"][filename].update(proof)
        if name in prepare.IMAGES:
            require(
                len({row["sha256"] for row in files[name].values()}) == 2,
                "image-clones-not-distinct",
            )
    del facts["activeInspection"]


def new_report():
    return {
        "schemaVersion": 1,
        "status": "failure",
        "reviewStatus": "UNREVIEWED",
        "scope": "read-only-image-inspection-not-product-vm-e2e",
        "packetGate": "independent-lineage-review-and-packet-hash-pin-required",
        "producer": {
            "organization": "mariner-org",
            "projectId": prepare.PROJECT_ID,
            "definitionId": 5073,
            "definitionRevision": 7,
            "buildId": BUILD,
            "sourceVersion": REVISION,
            "sourceBranch": REF,
        },
        "qualifiedProductionInputs": {
            "mic": {
                "identity": "mcr.microsoft.com/azurelinux/imagecustomizer",
                "sha256": MIC,
            },
            "base": BASE,
            "inclusiveTaskCitations": INPUT_CITES,
        },
        "complete": {},
        "unavailable": [],
        "failure": None,
    }


def write_report(output, report):
    text = json.dumps(report, indent=2) + "\n"
    if isinstance(output, int):
        # Privileged writes must never resolve the publication pathname again.
        with os.fdopen(os.dup(output), "w", encoding="utf-8") as target:
            target.seek(0)
            target.write(text)
            target.flush()
            target.truncate()
    else:
        output.write_text(text, encoding="utf-8")


def failed_publication(output, fact):
    report = new_report()
    report["failure"] = {"kind": "Failure", "missingFact": fact}
    report["unavailable"] = [fact]
    write_report(output, report)


def prepare_publication(output):
    require(output.name == "inspection.json", "invalid-publication-file")
    output.parent.mkdir(mode=0o700, exist_ok=False)
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        failed_publication(fd, "privileged-inspection-not-started")
    finally:
        os.close(fd)


def ensure_guestfish():
    if not shutil.which("guestfish"):
        require(os.geteuid() == 0, "guestfish-bootstrap-requires-root")
        command(
            ["apt-get", "-o", "DPkg::Lock::Timeout=60", "update"],
            timeout=180,
            failure_fact="guestfish-package-index-failed",
        )
        command(
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
            timeout=300,
            failure_fact="guestfish-package-install-failed",
        )
    require(shutil.which("guestfish"), "guestfish-missing-after-bootstrap")
    command(
        ["guestfish", "--version"],
        timeout=30,
        failure_fact="guestfish-readiness-failed",
    )


def execute(output, scratch, bootstrap_guestfish=False):
    report = new_report()
    code = 1
    try:
        require(
            sys.platform == "linux"
            and os.environ.get("EXCLUSIVE_DISPOSABLE_1ES") == "true",
            "authorized-disposable-linux-1es-required",
        )
        if bootstrap_guestfish:
            ensure_guestfish()
        missing = [name for name in COMMANDS if not shutil.which(name)]
        if missing:
            report["unavailable"] = ["command:" + name for name in missing]
            raise Failure("missing-inspection-commands")
        report["complete"]["toolIdentity"] = {
            name: measure(Path(shutil.which(name)).resolve())["sha256"]
            for name in COMMANDS
        }
        report["complete"]["harnessSha256"] = measure(Path(__file__))["sha256"]
        revision = command(["git", "rev-parse", "HEAD"]).strip()
        require(re.fullmatch(r"[0-9a-f]{40}", revision), "harness-revision-unavailable")
        report["complete"]["harnessRevision"] = revision
        extension = command(
            [
                "az",
                "extension",
                "show",
                "--name",
                "azure-devops",
                "--query",
                "path",
                "-o",
                "tsv",
            ],
            timeout=30,
            failure_fact="missing-azure-devops-cli-extension",
        ).strip()
        require(Path(extension).is_dir(), "missing-azure-devops-cli-extension")
        require(
            not any(
                os.environ.get(name)
                for name in (
                    "AZURE_DEVOPS_EXT_PAT",
                    "SYSTEM_ACCESSTOKEN",
                    "AZURE_ACCESS_TOKEN",
                    "AZURE_CLIENT_SECRET",
                    "AZURE_FEDERATED_TOKEN_FILE",
                )
            ),
            "ambient-alternate-credential-forbidden",
        )
        with tempfile.TemporaryDirectory(
            dir=scratch, prefix="trident-inspection-"
        ) as directory:
            # Private credential cache, downloads, logs, RPMDB copies and appliance state.
            with tempfile.TemporaryDirectory(dir=directory, prefix="home-") as home:
                os.environ.update(
                    HOME=home,
                    AZURE_CONFIG_DIR=str(Path(home) / ".azure"),
                    AZURE_EXTENSION_USE_DYNAMIC_INSTALL="no",
                    LIBGUESTFS_BACKEND="direct",
                    LIBGUESTFS_DEBUG="0",
                    LIBGUESTFS_TRACE="0",
                    AZURE_EXTENSION_DIR=str(Path(extension).parent),
                    LIBGUESTFS_CACHEDIR=directory,
                    LIBGUESTFS_TMPDIR=directory,
                    TMPDIR=directory,
                )
                command(
                    [
                        "az",
                        "login",
                        "--identity",
                        "--client-id",
                        "e2fd61e6-b160-4f02-a5ac-3d949a4fe01d",
                        "--allow-no-subscriptions",
                        "--output",
                        "none",
                    ],
                    timeout=120,
                    failure_fact="runner-umi-login-failed",
                )
                inspect(Path(directory), report["complete"])
        report["status"] = "complete"
        code = 0
    except Cancelled:
        report["failure"] = {"kind": "cancelled"}
        code = 130
    except subprocess.TimeoutExpired:
        report["failure"] = {
            "kind": "timeout",
            "missingFact": "inspection-command-timeout",
        }
    except Exception as error:
        # Shared helper/tool exceptions can contain signed URLs or log bodies.
        report["failure"] = {
            "kind": (
                type(error).__name__
                if type(error) in (Failure, OSError, ValueError)
                else "inspection-failure"
            ),
            "missingFact": (
                str(error) if type(error) is Failure else "inspection-did-not-complete"
            ),
        }
        status = re.search(r"\bHTTP ([0-9]{3})\b", str(error))
        if status:
            report["failure"]["httpStatus"] = int(status[1])
    finally:
        active = report["complete"].pop("activeInspection", None)
        if active and report["failure"]:
            report["failure"]["measurement"] = active
        if code:
            report["unavailable"].append("qualification-incomplete-see-primary-failure")
        write_report(output, report)
    return code


def privileged_inspection(args):
    require(os.geteuid() == 0, "privileged-inspection-requires-root")
    require(
        args.output.name == "inspection.json"
        and args.publication_uid is not None
        and args.publication_uid >= 0
        and args.publication_gid is not None
        and args.publication_gid >= 0,
        "invalid-publication-owner",
    )
    directory = os.open(
        args.output.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    )
    try:
        original_directory = os.fstat(directory)
        report_file = os.open(
            args.output.name,
            os.O_WRONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
            dir_fd=directory,
        )
        try:
            original = os.fstat(report_file)
            for fd, mode in ((directory, 0o700), (report_file, 0o600)):
                info = os.fstat(fd)
                require(
                    (info.st_uid, info.st_gid)
                    == (args.publication_uid, args.publication_gid)
                    and stat.S_IMODE(info.st_mode) == mode,
                    "publication-not-private-and-job-owned",
                )
            require(
                stat.S_ISREG(original.st_mode) and original.st_nlink == 1,
                "invalid-publication-file",
            )
            code = 1
            inspected = False
            try:
                with tempfile.TemporaryDirectory(
                    dir=args.scratch,
                    prefix="trident-root-context-",
                ) as home:
                    os.environ.update(
                        HOME=home,
                        AZURE_CONFIG_DIR=str(Path(home) / ".azure"),
                        AZURE_EXTENSION_USE_DYNAMIC_INSTALL="no",
                        LIBGUESTFS_BACKEND="direct",
                        LIBGUESTFS_DEBUG="0",
                        LIBGUESTFS_TRACE="0",
                        GIT_CONFIG_COUNT="1",
                        GIT_CONFIG_KEY_0="safe.directory",
                        GIT_CONFIG_VALUE_0=str(Path(__file__).resolve().parents[2]),
                    )
                    code = execute(report_file, args.scratch, bootstrap_guestfish=True)
                    inspected = True
            except Cancelled:
                if not inspected or code == 0:
                    failed_publication(report_file, "privileged-inspection-cancelled")
                    code = 130
            except Exception:
                if not inspected or code == 0:
                    failed_publication(
                        report_file, "privileged-inspection-setup-failed"
                    )
                code = code or 1
            finally:
                try:
                    current_directory = os.stat(
                        args.output.parent, follow_symlinks=False
                    )
                    require(
                        (current_directory.st_dev, current_directory.st_ino)
                        == (original_directory.st_dev, original_directory.st_ino),
                        "publication-directory-replaced",
                    )
                    current = os.stat(args.output, follow_symlinks=False)
                    require(
                        (current.st_dev, current.st_ino)
                        == (original.st_dev, original.st_ino),
                        "publication-file-replaced",
                    )
                    for fd, mode in ((report_file, 0o600), (directory, 0o700)):
                        os.fchmod(fd, mode)
                        os.fchown(fd, args.publication_uid, args.publication_gid)
                except (OSError, Failure):
                    print("Exact report publication failed", file=sys.stderr)
                    if code == 0:
                        code = 1
                        failed_publication(
                            report_file, "exact-report-publication-failed"
                        )
            return code
        finally:
            os.close(report_file)
    finally:
        os.close(directory)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--prepare-publication", action="store_true")
    parser.add_argument("--bootstrap-guestfish", action="store_true")
    parser.add_argument("--publication-uid", type=int)
    parser.add_argument("--publication-gid", type=int)
    args = parser.parse_args()
    os.umask(0o077)
    if args.prepare_publication:
        prepare_publication(args.output)
        return 0
    if not args.bootstrap_guestfish:
        args.output.parent.mkdir(parents=True, exist_ok=False)
    stopping = False

    def cancel(signum, frame):
        nonlocal stopping
        if not stopping:
            stopping = True
            raise Cancelled()

    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, cancel)
    if args.bootstrap_guestfish:
        try:
            return privileged_inspection(args)
        except Exception:
            print("Private privileged inspection setup failed", file=sys.stderr)
            return 1
    return execute(args.output, args.scratch)


if __name__ == "__main__":
    sys.exit(main())
