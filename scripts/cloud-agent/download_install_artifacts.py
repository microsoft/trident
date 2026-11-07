#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import sys
import tempfile
import urllib.parse
import zipfile

from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any, Dict, List

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from download_functional_artifact import (  # noqa: E402
    CHUNK_SIZE,
    ArtifactError,
    build_api_url,
    create_temporary_path,
    download_file,
    ensure_link,
    get_ado_token,
    hash_file,
    request_json,
    verify_artifact,
    verify_build,
)


def load_config(path: Path) -> Dict[str, Any]:
    with path.open(encoding="utf-8") as config_file:
        config = json.load(config_file)
    if config.get("schemaVersion") != 1:
        raise ArtifactError(f"Unsupported configuration schema in {path}")
    artifacts = config.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        raise ArtifactError("Install artifact configuration must contain artifacts")

    cache_filenames = set()
    destination_paths = set()
    for artifact in artifacts:
        for field in (
            "artifactId",
            "artifactName",
            "artifactType",
            "artifactSize",
            "archivePath",
            "payloadSize",
            "payloadSha256",
            "cacheFilename",
            "destinationPath",
        ):
            if field not in artifact:
                raise ArtifactError(
                    f"Install artifact {artifact.get('artifactName', '<unknown>')} "
                    f"is missing {field}"
                )
        cache_filename = artifact["cacheFilename"]
        destination_path = artifact["destinationPath"]
        if Path(cache_filename).name != cache_filename:
            raise ArtifactError(f"cacheFilename must be a filename: {cache_filename}")
        if Path(destination_path).is_absolute() or ".." in Path(destination_path).parts:
            raise ArtifactError(
                f"destinationPath must stay within the repository: {destination_path}"
            )
        if cache_filename in cache_filenames:
            raise ArtifactError(f"Duplicate cacheFilename: {cache_filename}")
        if destination_path in destination_paths:
            raise ArtifactError(f"Duplicate destinationPath: {destination_path}")
        cache_filenames.add(cache_filename)
        destination_paths.add(destination_path)
    return config


def payload_matches(config: Dict[str, Any], path: Path) -> bool:
    return (
        path.is_file()
        and path.stat().st_size == config["payloadSize"]
        and hash_file(path) == config["payloadSha256"]
    )


def find_payload_entry(
    archive: zipfile.ZipFile, expected_archive_path: str
) -> zipfile.ZipInfo:
    expected = PurePosixPath(expected_archive_path)
    if expected.is_absolute() or ".." in expected.parts:
        raise ArtifactError(f"Unsafe configured archive path: {expected_archive_path}")

    matches = []
    for entry in archive.infolist():
        path = PurePosixPath(entry.filename)
        if path.is_absolute() or ".." in path.parts:
            raise ArtifactError(f"Unsafe path in artifact ZIP: {entry.filename}")
        if not entry.is_dir() and path == expected:
            matches.append(entry)
    if len(matches) != 1:
        raise ArtifactError(
            f"Expected exactly one {expected_archive_path} in artifact ZIP, "
            f"found {len(matches)}"
        )
    return matches[0]


def extract_payload(
    config: Dict[str, Any], archive_path: Path, payload_path: Path
) -> None:
    temporary_path = create_temporary_path(
        payload_path.parent,
        prefix=f".{payload_path.name}.",
        suffix=".tmp",
    )
    try:
        with zipfile.ZipFile(archive_path) as archive:
            entry = find_payload_entry(archive, config["archivePath"])
            if entry.file_size != config["payloadSize"]:
                raise ArtifactError(
                    f"Payload size mismatch: expected {config['payloadSize']}, "
                    f"got {entry.file_size}"
                )

            digest = hashlib.sha256()
            with archive.open(entry) as source, temporary_path.open("wb") as output:
                while chunk := source.read(CHUNK_SIZE):
                    output.write(chunk)
                    digest.update(chunk)

        actual_hash = digest.hexdigest()
        if actual_hash != config["payloadSha256"]:
            raise ArtifactError(
                f"Payload SHA-256 mismatch: expected {config['payloadSha256']}, "
                f"got {actual_hash}"
            )
        os.replace(temporary_path, payload_path)
    finally:
        temporary_path.unlink(missing_ok=True)


def verify_artifact_id(config: Dict[str, Any], artifact: Dict[str, Any]) -> None:
    if artifact.get("id") != config["artifactId"]:
        raise ArtifactError(
            "Pinned artifact metadata mismatch:\n"
            f"artifact.id: expected {config['artifactId']!r}, "
            f"got {artifact.get('id')!r}"
        )


def write_provenance(
    config: Dict[str, Any],
    build: Dict[str, Any],
    prepared_artifacts: List[Dict[str, Any]],
    provenance_path: Path,
) -> None:
    provenance = {
        "schemaVersion": 1,
        "verifiedAt": datetime.now(timezone.utc).isoformat(),
        "build": {
            "id": build["id"],
            "number": build.get("buildNumber"),
            "definitionId": build["definition"]["id"],
            "result": build["result"],
            "sourceBranch": build["sourceBranch"],
            "sourceVersion": build["sourceVersion"],
        },
        "artifacts": prepared_artifacts,
    }
    temporary_path = create_temporary_path(
        provenance_path.parent,
        prefix=f".{provenance_path.name}.",
        suffix=".tmp",
    )
    try:
        with temporary_path.open("w", encoding="utf-8") as provenance_file:
            json.dump(provenance, provenance_file, indent=2)
            provenance_file.write("\n")
        os.replace(temporary_path, provenance_path)
    finally:
        temporary_path.unlink(missing_ok=True)


def prepare_artifacts(
    config: Dict[str, Any],
    output_root: Path,
    repository_root: Path,
    token: str,
) -> Path:
    output_root = output_root.expanduser().resolve()
    repository_root = repository_root.expanduser().resolve()
    build_id = config["buildId"]
    build_url = build_api_url(config, f"build/builds/{build_id}?api-version=7.1")
    build = request_json(build_url, token)
    verify_build(config, build)

    output_dir = output_root / f"build-{build_id}"
    output_dir.mkdir(parents=True, exist_ok=True)
    prepared_artifacts = []

    for artifact_config in config["artifacts"]:
        artifact_query = urllib.parse.urlencode(
            {
                "artifactName": artifact_config["artifactName"],
                "api-version": "7.1",
            }
        )
        artifact_url = build_api_url(
            config, f"build/builds/{build_id}/artifacts?{artifact_query}"
        )
        artifact = request_json(artifact_url, token)
        download_url = verify_artifact({**config, **artifact_config}, artifact)
        verify_artifact_id(artifact_config, artifact)

        payload_path = output_dir / artifact_config["cacheFilename"]
        cache_hit = payload_matches(artifact_config, payload_path)
        if not cache_hit:
            if payload_path.exists():
                raise ArtifactError(
                    f"Cached payload does not match pinned provenance: {payload_path}"
                )
            with tempfile.NamedTemporaryFile(
                dir=output_dir, prefix="artifact-", suffix=".zip", delete=False
            ) as temporary_file:
                archive_path = Path(temporary_file.name)
            try:
                download_file(download_url, token, archive_path)
                extract_payload(artifact_config, archive_path, payload_path)
            finally:
                archive_path.unlink(missing_ok=True)

        destination_path = repository_root / artifact_config["destinationPath"]
        ensure_link(payload_path, destination_path)
        prepared_artifacts.append(
            {
                "id": artifact["id"],
                "name": artifact["name"],
                "type": artifact["resource"]["type"],
                "size": int(artifact["resource"]["properties"]["artifactsize"]),
                "cacheHit": cache_hit,
                "payload": {
                    "archivePath": artifact_config["archivePath"],
                    "path": str(payload_path),
                    "destinationPath": str(destination_path),
                    "size": payload_path.stat().st_size,
                    "sha256": artifact_config["payloadSha256"],
                },
            }
        )

    provenance_path = output_dir / "provenance.json"
    write_provenance(config, build, prepared_artifacts, provenance_path)
    return provenance_path


def parse_args() -> argparse.Namespace:
    repository_root = SCRIPT_DIR.parent.parent
    parser = argparse.ArgumentParser(
        description="Download and verify the pinned Trident install artifacts."
    )
    parser.add_argument(
        "--config",
        type=Path,
        default=SCRIPT_DIR / "install-artifacts.json",
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=Path(
            os.environ.get(
                "TRIDENT_INSTALL_CACHE_DIR",
                "/mnt/storage/trident-cloud-agent/install",
            )
        ),
    )
    parser.add_argument(
        "--repository-root",
        type=Path,
        default=repository_root,
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    provenance_path = prepare_artifacts(
        load_config(args.config),
        args.output_root,
        args.repository_root,
        get_ado_token(),
    )
    print(provenance_path)


if __name__ == "__main__":
    main()
