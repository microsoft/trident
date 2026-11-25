#!/usr/bin/env python3

import argparse
import hashlib
import importlib.util
import json
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile

from pathlib import Path, PurePosixPath

import download_functional_artifact as ado

PROJECT_ID = "2311650c-e79e-4301-b4d2-96543fdd84ff"
TOOLS = ("netlaunch", "netlisten", "mkcosi", "storm-trident", "virtdeploy")
INSTALLERS = ("trident-installer", "trident-split-installer")
IMAGES = {
    "trident-testimage": "regular",
    "trident-verity-testimage": "verity",
    "trident-usrverity-testimage": "usrverity",
}
STAGES = {
    "go-tools": "BuildingTools",
    "trident-binaries": "GetTridentBinaries_rpms_amd64",
    **{
        name: "TridentTestImg_" + name.replace("-", "_")
        for name in (*INSTALLERS, *IMAGES)
    },
}


def require(condition, message):
    if not condition:
        raise ado.ArtifactError(message)


def sha256(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value)


def keys(value, expected):
    require(
        isinstance(value, dict) and set(value) == set(expected),
        "Unexpected or missing evidence fields (raw logs are forbidden)",
    )


def identity(value):
    require(
        isinstance(value, str)
        and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/@:+-]*", value),
        "Expected minimal resolved identity, no query strings or free-form text",
    )


def reader():
    spec = importlib.util.spec_from_file_location(
        "vm_install", Path(__file__).with_name("run-vm-e2e-install.py")
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def producer(args):
    require(
        type(args.build_id) is int and args.build_id > 0, "Explicit build ID required"
    )
    require(
        args.expected_definition_id == 5073, "Expected producer must be definition 5073"
    )
    require(
        re.fullmatch(r"[0-9a-f]{40}", args.expected_revision),
        "Full candidate SHA required",
    )
    require(
        re.fullmatch(r"refs/[A-Za-z0-9._/-]+", args.expected_ref),
        "Explicit candidate ref required",
    )
    return {
        "organization": "mariner-org",
        "projectId": PROJECT_ID,
        "definitionId": args.expected_definition_id,
        "buildId": args.build_id,
        "sourceVersion": args.expected_revision,
        "sourceBranch": args.expected_ref,
    }


def verify_server(config, expected, token):
    build = ado.resolve_build(config, token, expected["buildId"])
    require(
        build["sourceVersion"] == expected["sourceVersion"], "Producer source mismatch"
    )
    require(
        build.get("project", {}).get("id") == PROJECT_ID, "Producer project mismatch"
    )
    timeline = ado.request_json(
        ado.build_api_url(
            config, f"build/builds/{expected['buildId']}/timeline?api-version=7.1"
        ),
        token,
    )
    records = timeline["records"]
    stages = {}
    for name in STAGES.values():
        matches = [
            row
            for row in records
            if row.get("type") == "Stage" and row.get("identifier") == name
        ]
        require(len(matches) == 1, f"Missing or ambiguous producer stage: {name}")
        stage = matches[0]
        require(
            stage.get("state") == "completed" and stage.get("result") == "succeeded",
            f"Producer stage did not succeed: {name}",
        )
        stages[name] = stage["id"]
    log_metadata = ado.request_json(
        ado.build_api_url(
            config, f"build/builds/{expected['buildId']}/logs?api-version=7.1"
        ),
        token,
    )["value"]
    return build, records, stages, log_metadata


def verify_evidence(args, expected, records, stages, log_metadata):
    evidence = args.evidence.read_bytes()
    require(
        sha256(args.evidence_sha256)
        and hashlib.sha256(evidence).hexdigest() == args.evidence_sha256,
        "Reviewed evidence packet hash mismatch",
    )
    packet = json.loads(evidence)
    keys(packet, ("schemaVersion", "producer", "review", "logs", "rpms", "artifacts"))
    require(
        packet["schemaVersion"] == 1 and packet["producer"] == expected,
        "Evidence producer/source mismatch",
    )
    keys(packet["review"], ("reviewer", "reference"))
    for field in ("reviewer", "reference"):
        identity(packet["review"][field])
    by_id = {row["id"]: row for row in records}
    logs = packet["logs"]
    require(logs, "Missing reviewed producer log identities")
    for log_id, log in logs.items():
        keys(log, ("sha256", "lineCount"))
        require(sha256(log["sha256"]), "Invalid reviewed log hash")
        matches = [item for item in log_metadata if str(item["id"]) == log_id]
        require(
            len(matches) == 1
            and type(log["lineCount"]) is int
            and log["lineCount"] > 0
            and log["lineCount"] == matches[0]["lineCount"],
            "Reviewed log metadata mismatch",
        )

    def citation(value, artifact):
        keys(value, ("logId", "startLine", "endLine"))
        log = logs[str(value["logId"])]
        require(
            type(value["logId"]) is int
            and type(value["startLine"]) is int
            and type(value["endLine"]) is int
            and 1 <= value["startLine"] <= value["endLine"] <= log["lineCount"],
            "Invalid producer log citation",
        )
        matches = [
            row for row in records if (row.get("log") or {}).get("id") == value["logId"]
        ]
        require(len(matches) == 1, "Evidence log is not in producer timeline")
        row = matches[0]
        seen = set()
        while row["id"] != stages[STAGES[artifact]]:
            require(row["id"] not in seen, "Cyclic producer timeline")
            seen.add(row["id"])
            require(
                row.get("state") == "completed" and row.get("result") == "succeeded",
                "Evidence task/job did not succeed",
            )
            row = by_id[row["parentId"]]

    keys(packet["rpms"], ("checkoutRevision", "packages", "evidence"))
    packages = packet["rpms"]["packages"]
    require(packages, "Missing candidate RPM identities")
    for package in packages:
        keys(package, ("name", "nevra", "sha256"))
        identity(package["name"])
        identity(package["nevra"])
        require(
            package["name"] and package["nevra"] and sha256(package["sha256"]),
            "Invalid candidate RPM identity",
        )
    require(any(p["name"] == "trident" for p in packages), "Missing trident RPM")
    require(
        len({p["name"] for p in packages}) == len(packages),
        "Ambiguous candidate RPM identities",
    )
    require(
        packet["rpms"]["checkoutRevision"] == expected["sourceVersion"],
        "RPM checkout mismatch",
    )
    citation(packet["rpms"]["evidence"], "trident-binaries")
    artifacts = packet["artifacts"]
    require(
        set(artifacts) == set(STAGES) - {"trident-binaries"},
        "Evidence artifact set mismatch",
    )
    for name, artifact in artifacts.items():
        fields = ("checkoutRevision", "checkoutEvidence", "files")
        if name != "go-tools":
            fields += ("rpmBuildId", "rpmArtifact", "rpmEvidence", "baseVhdx", "mic")
        keys(artifact, fields)
        require(
            artifact["checkoutRevision"] == expected["sourceVersion"],
            f"Candidate checkout not proven: {name}",
        )
        citation(artifact["checkoutEvidence"], name)
        filenames = (
            set(TOOLS)
            if name == "go-tools"
            else (
                {name + ".iso"}
                if name in INSTALLERS
                else {f"{name}_{index}.cosi" for index in range(2)}
            )
        )
        if args.include_uki_certificate and name == "trident-usrverity-testimage":
            filenames.add("ca_cert.pem")
        require(
            set(artifact["files"]) == filenames, f"Missing clone/file evidence: {name}"
        )
        if name != "go-tools":
            require(
                artifact["rpmBuildId"] == expected["buildId"]
                and artifact["rpmArtifact"] == "trident-binaries",
                f"Current-run RPM consumption not proven: {name}",
            )
            citation(artifact["rpmEvidence"], name)
            for kind in ("baseVhdx", "mic"):
                resolved = artifact[kind]
                keys(resolved, ("identity", "sha256", "evidence"))
                identity(resolved["identity"])
                require(
                    sha256(resolved["sha256"]),
                    f"Unresolved producer {kind}: {name}",
                )
                citation(resolved["evidence"], name)
        for filename, metadata in artifact["files"].items():
            fields = ("size", "sha256")
            if name != "go-tools" and filename != "ca_cert.pem":
                fields += ("installedPackages", "installEvidence")
            keys(metadata, fields)
            require(
                type(metadata["size"]) is int
                and metadata["size"] > 0
                and sha256(metadata["sha256"]),
                "Invalid reviewed file identity",
            )
            if name != "go-tools" and filename != "ca_cert.pem":
                installed = metadata["installedPackages"]
                required = "trident" if name in INSTALLERS else "trident-service"
                require(
                    installed
                    and all(p in packages for p in installed)
                    and any(p["name"] == required for p in installed),
                    f"Installed candidate RPM identity not proven: {filename}",
                )
                citation(metadata["installEvidence"], name)
    return packet


def extract(archive_path, destination, files):
    # Reuse B's single-entry extraction, adding whole-archive checks because these
    # artifacts contain multiple files and must never introduce special paths.
    with zipfile.ZipFile(archive_path) as archive:
        for entry in archive.infolist():
            path = PurePosixPath(entry.filename)
            mode = entry.external_attr >> 16
            require(
                entry.filename
                and "\\" not in entry.filename
                and ":" not in entry.filename
                and not path.is_absolute()
                and ".." not in path.parts
                and (stat.S_IFMT(mode) in (0, stat.S_IFREG, stat.S_IFDIR)),
                f"Unsafe artifact ZIP entry: {entry.filename}",
            )
        for filename, metadata in files.items():
            require(
                ado.find_image_entry(archive, filename).file_size == metadata["size"],
                f"Reviewed ZIP entry size mismatch: {filename}",
            )
    destination.mkdir()
    for filename, metadata in files.items():
        path = destination / filename
        ado.extract_image({"imageFilename": filename}, archive_path, path)
        require(
            ado.image_matches(path, metadata["size"], metadata["sha256"]),
            f"Reviewed artifact hash/size mismatch: {filename}",
        )


def prepare(args, token):
    expected = producer(args)
    config = {
        "organization": expected["organization"],
        "project": PROJECT_ID,
        "definitionId": expected["definitionId"],
        "sourceBranch": expected["sourceBranch"],
        "buildStatus": "completed",
        "buildResult": "succeeded",
    }
    build, records, stages, log_metadata = verify_server(config, expected, token)
    packet = verify_evidence(args, expected, records, stages, log_metadata)
    listing = ado.request_json(
        ado.build_api_url(
            config, f"build/builds/{args.build_id}/artifacts?api-version=7.1"
        ),
        token,
    )["value"]
    selected = {}
    for name in packet["artifacts"]:
        matches = [artifact for artifact in listing if artifact.get("name") == name]
        require(len(matches) == 1, f"Missing or ambiguous artifact: {name}")
        artifact = matches[0]
        require(
            type(artifact.get("id")) is int and artifact["id"] > 0,
            f"Missing immutable artifact ID: {name}",
        )
        url = ado.verify_artifact(
            {"artifactName": name, "artifactType": "PipelineArtifact"}, artifact
        )
        selected[name] = (artifact, url)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    bundle = output / "bundle"
    bundle.mkdir()
    files = {}
    downloads = {}

    def inventory(path, artifact, source):
        require(
            path.is_file() and not path.is_symlink(), f"Missing prepared file: {path}"
        )
        metadata = packet["artifacts"][artifact]["files"][source]
        require(
            ado.image_matches(path, metadata["size"], metadata["sha256"]),
            f"Preparation changed reviewed image: {path}",
        )
        files[path.relative_to(bundle).as_posix()] = {
            "artifact": artifact,
            "sourceFile": source,
            "size": path.stat().st_size,
            "sha256": ado.hash_file(path),
        }

    with tempfile.TemporaryDirectory(dir=output, prefix=".download-") as temp:
        staging = Path(temp)
        for name, (artifact, url) in selected.items():
            archive = staging / f"{name}.zip"
            ado.download_file(url, token, archive)
            downloads[name] = {
                "id": artifact["id"],
                "size": ado.artifact_size(artifact),
                "zipSize": archive.stat().st_size,
                "zipSha256": ado.hash_file(archive),
            }
            extract(archive, staging / name, packet["artifacts"][name]["files"])
        shutil.move(str(staging / "go-tools"), bundle / "bin")
        for tool in TOOLS:
            path = bundle / "bin" / tool
            path.chmod(0o755)
            inventory(path, "go-tools", tool)
        iso = bundle / "artifacts/iso"
        iso.mkdir(parents=True)
        for name in INSTALLERS:
            path = iso / f"{name}.iso"
            shutil.move(str(staging / name / path.name), path)
            inventory(path, name, path.name)
        target = bundle / "artifacts/test-image"
        command = [
            str(bundle / "bin/storm-trident"),
            "helper",
            "prepare-images",
            "-a",
            "--",
            *(str(staging / name) for name in IMAGES),
            *IMAGES,
            str(target),
            "-v",
            "4",
        ]
        with (output / "prepare-images.log").open("wb") as log:
            subprocess.run(
                command,
                cwd=output,
                stdout=log,
                stderr=subprocess.STDOUT,
                check=True,
                timeout=args.timeout_seconds,
            )
        for name, stem in IMAGES.items():
            for index in range(4):
                filename = stem + (f"_v{index + 1}" if index else "") + ".cosi"
                inventory(target / filename, name, f"{name}_{index % 2}.cosi")
            require(
                files[f"artifacts/test-image/{stem}.cosi"]["sha256"]
                != files[f"artifacts/test-image/{stem}_v2.cosi"]["sha256"],
                f"Clones must be distinct: {name}",
            )
        certificate = None
        if args.include_uki_certificate:
            certificate = "artifacts/usrverity/ca_cert.pem"
            path = bundle / certificate
            path.parent.mkdir()
            shutil.move(str(staging / "trident-usrverity-testimage/ca_cert.pem"), path)
            inventory(path, "trident-usrverity-testimage", "ca_cert.pem")
    manifest = {
        "schemaVersion": 1,
        "candidateRevision": args.expected_revision,
        "producer": expected,
        "files": files,
        "lineage": {
            "basis": "independently-reviewed-pinned-evidence",
            "packetSha256": args.evidence_sha256,
            "packet": packet,
        },
        "resolution": {
            "artifacts": downloads,
            "buildStatus": build["status"],
            "buildResult": build["result"],
            "stages": {
                name: {"id": stage_id, "state": "completed", "result": "succeeded"}
                for name, stage_id in stages.items()
            },
        },
    }
    if certificate:
        manifest["signingCertificate"] = certificate
    path = output / "provenance.pending.json"
    path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    pin = ado.hash_file(path)
    reader().verify_bundle(bundle, path, pin, args.expected_revision)
    path = path.rename(bundle / "provenance.json")
    return path, pin


def main():
    parser = argparse.ArgumentParser(
        description="Prepare one qualified candidate ADO image set"
    )
    parser.add_argument("--build-id", type=ado.positive_int, required=True)
    parser.add_argument(
        "--expected-definition-id", type=ado.positive_int, required=True
    )
    parser.add_argument("--expected-revision", required=True)
    parser.add_argument("--expected-ref", required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--evidence-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--include-uki-certificate", action="store_true")
    parser.add_argument(
        "--timeout-seconds", type=reader().positive_seconds, required=True
    )
    args = parser.parse_args()
    try:
        producer(args)
        path, pin = prepare(args, ado.get_ado_token())
        print(json.dumps({"manifest": str(path), "sha256": pin}))
        return 0
    except (
        ado.ArtifactError,
        OSError,
        ValueError,
        KeyError,
        TypeError,
        subprocess.SubprocessError,
        zipfile.BadZipFile,
    ) as error:
        # Shared helper exceptions can contain signed URLs or untrusted values.
        status = re.search(r"\bHTTP ([0-9]{3})\b", str(error))
        detail = f", HTTP {status[1]}" if status else ""
        print(
            f"Artifact preparation failed ({type(error).__name__}{detail})",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    sys.exit(main())
