#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile

from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any, Dict, Optional, Tuple

ADO_RESOURCE = "499b84ac-1321-427f-aa17-267ca6975798"
CHUNK_SIZE = 1024 * 1024


class ArtifactError(RuntimeError):
    pass


def load_config(path: Path) -> Dict[str, Any]:
    with path.open(encoding="utf-8") as config_file:
        config = json.load(config_file)
    if config.get("schemaVersion") != 2:
        raise ArtifactError(f"Unsupported configuration schema in {path}")
    return config


def get_ado_token() -> str:
    az = shutil.which("az") or shutil.which("az.cmd")
    if not az:
        raise ArtifactError("Azure CLI is not available on PATH")
    result = subprocess.run(
        [
            az,
            "account",
            "get-access-token",
            "--resource",
            ADO_RESOURCE,
            "--query",
            "accessToken",
            "-o",
            "tsv",
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    token = result.stdout.strip()
    if not token:
        raise ArtifactError("Azure CLI returned an empty Azure DevOps token")
    return token


def request_json(url: str, token: str) -> Dict[str, Any]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/json",
            "Authorization": f"Bearer {token}",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise ArtifactError(
            f"Azure DevOps request failed with HTTP {error.code}: {url}"
        ) from error
    except urllib.error.URLError as error:
        raise ArtifactError(f"Azure DevOps request failed: {error.reason}") from error


def build_api_url(config: Dict[str, Any], suffix: str) -> str:
    organization = urllib.parse.quote(config["organization"], safe="")
    project = urllib.parse.quote(config["project"], safe="")
    return f"https://dev.azure.com/{organization}/{project}/_apis/{suffix}"


def verify_build(config: Dict[str, Any], build: Dict[str, Any]) -> None:
    expected = {
        "definition.id": config["definitionId"],
        "status": config["buildStatus"],
        "result": config["buildResult"],
        "sourceBranch": config["sourceBranch"],
    }
    actual = {
        "definition.id": build.get("definition", {}).get("id"),
        "status": build.get("status"),
        "result": build.get("result"),
        "sourceBranch": build.get("sourceBranch"),
    }
    mismatches = [
        f"{key}: expected {expected[key]!r}, got {actual[key]!r}"
        for key in expected
        if actual[key] != expected[key]
    ]
    if mismatches:
        raise ArtifactError("Build metadata mismatch:\n" + "\n".join(mismatches))
    source_version = build.get("sourceVersion")
    if not isinstance(source_version, str) or not source_version:
        raise ArtifactError(
            f"Build metadata contains an invalid source version: {source_version!r}"
        )


def resolve_build(
    config: Dict[str, Any], token: str, build_id: Optional[int]
) -> Dict[str, Any]:
    if build_id is not None:
        build_url = build_api_url(config, f"build/builds/{build_id}?api-version=7.1")
        build = request_json(build_url, token)
        if build.get("id") != build_id:
            raise ArtifactError(
                f"Build ID mismatch: expected {build_id!r}, got {build.get('id')!r}"
            )
    else:
        build_query = urllib.parse.urlencode(
            {
                "definitions": config["definitionId"],
                "branchName": config["sourceBranch"],
                "statusFilter": config["buildStatus"],
                "resultFilter": config["buildResult"],
                "queryOrder": "finishTimeDescending",
                "$top": 1,
                "api-version": "7.1",
            }
        )
        builds_url = build_api_url(config, f"build/builds?{build_query}")
        response = request_json(builds_url, token)
        builds = response.get("value")
        if not isinstance(builds, list) or not builds:
            raise ArtifactError(
                "No successful completed functional-test build was found"
            )
        build = builds[0]

    verify_build(config, build)
    if not isinstance(build.get("id"), int):
        raise ArtifactError(
            f"Build metadata contains an invalid ID: {build.get('id')!r}"
        )
    return build


def artifact_size(artifact: Dict[str, Any]) -> int:
    value = artifact.get("resource", {}).get("properties", {}).get("artifactsize")
    try:
        size = int(value)
    except (TypeError, ValueError) as error:
        raise ArtifactError(
            f"Artifact metadata contains an invalid size: {value!r}"
        ) from error
    if size <= 0:
        raise ArtifactError(f"Artifact metadata contains an invalid size: {value!r}")
    return size


def verify_artifact(config: Dict[str, Any], artifact: Dict[str, Any]) -> str:
    resource = artifact.get("resource", {})
    expected = {
        "name": config["artifactName"],
        "resource.type": config["artifactType"],
    }
    actual = {
        "name": artifact.get("name"),
        "resource.type": resource.get("type"),
    }
    mismatches = [
        f"{key}: expected {expected[key]!r}, got {actual[key]!r}"
        for key in expected
        if actual[key] != expected[key]
    ]
    if mismatches:
        raise ArtifactError("Artifact metadata mismatch:\n" + "\n".join(mismatches))
    artifact_size(artifact)

    download_url = resource.get("downloadUrl")
    if not download_url:
        raise ArtifactError("Artifact metadata does not contain resource.downloadUrl")
    parsed_url = urllib.parse.urlparse(download_url)
    if (
        parsed_url.scheme != "https"
        or not parsed_url.hostname
        or not parsed_url.hostname.endswith(".artifacts.visualstudio.com")
    ):
        raise ArtifactError(f"Unexpected artifact download URL: {download_url}")
    return download_url


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(CHUNK_SIZE):
            digest.update(chunk)
    return digest.hexdigest()


def image_matches(path: Path, expected_size: int, expected_sha256: str) -> bool:
    return (
        path.is_file()
        and path.stat().st_size == expected_size
        and hash_file(path) == expected_sha256
    )


def download_file(url: str, token: str, destination: Path) -> None:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/zip, application/octet-stream",
            "Authorization": f"Bearer {token}",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=600) as response:
            content_type = response.headers.get_content_type()
            if content_type == "text/html":
                raise ArtifactError(
                    "Artifact download returned HTML instead of a ZIP archive"
                )
            with destination.open("wb") as output:
                shutil.copyfileobj(response, output, CHUNK_SIZE)
    except urllib.error.HTTPError as error:
        raise ArtifactError(
            f"Artifact download failed with HTTP {error.code}"
        ) from error
    except urllib.error.URLError as error:
        raise ArtifactError(f"Artifact download failed: {error.reason}") from error


def find_image_entry(
    archive: zipfile.ZipFile, expected_filename: str
) -> zipfile.ZipInfo:
    matches = []
    for entry in archive.infolist():
        path = PurePosixPath(entry.filename)
        if path.is_absolute() or ".." in path.parts:
            raise ArtifactError(f"Unsafe path in artifact ZIP: {entry.filename}")
        if not entry.is_dir() and path.name == expected_filename:
            matches.append(entry)
    if len(matches) != 1:
        raise ArtifactError(
            f"Expected exactly one {expected_filename} in artifact ZIP, "
            f"found {len(matches)}"
        )
    return matches[0]


def extract_image(
    config: Dict[str, Any], archive_path: Path, image_path: Path
) -> Tuple[int, str]:
    temporary_path = create_temporary_path(
        image_path.parent,
        prefix=f".{image_path.name}.",
        suffix=".tmp",
    )
    try:
        with zipfile.ZipFile(archive_path) as archive:
            entry = find_image_entry(archive, config["imageFilename"])

            digest = hashlib.sha256()
            with archive.open(entry) as source, temporary_path.open("wb") as output:
                while chunk := source.read(CHUNK_SIZE):
                    output.write(chunk)
                    digest.update(chunk)

        actual_size = temporary_path.stat().st_size
        if actual_size != entry.file_size:
            raise ArtifactError(
                f"Extracted image size mismatch: expected {entry.file_size}, "
                f"got {actual_size}"
            )
        os.replace(temporary_path, image_path)
        return actual_size, digest.hexdigest()
    finally:
        temporary_path.unlink(missing_ok=True)


def create_temporary_path(directory: Path, prefix: str, suffix: str) -> Path:
    descriptor, temporary_path = tempfile.mkstemp(
        dir=directory,
        prefix=prefix,
        suffix=suffix,
    )
    os.close(descriptor)
    return Path(temporary_path)


def ensure_link(image_path: Path, link_path: Path) -> None:
    link_path.parent.mkdir(parents=True, exist_ok=True)
    if link_path.is_symlink():
        if link_path.resolve() == image_path.resolve():
            return
    elif link_path.exists():
        raise ArtifactError(f"Refusing to replace existing path: {link_path}")

    with tempfile.TemporaryDirectory(
        dir=link_path.parent, prefix=f".{link_path.name}."
    ) as temporary_dir:
        temporary_link = Path(temporary_dir) / link_path.name
        temporary_link.symlink_to(image_path)
        os.replace(temporary_link, link_path)


def cached_image_metadata(
    config: Dict[str, Any],
    build: Dict[str, Any],
    artifact: Dict[str, Any],
    image_path: Path,
    provenance_path: Path,
) -> Optional[Tuple[int, str]]:
    if not image_path.exists() and not provenance_path.exists():
        return None
    if not image_path.is_file() or not provenance_path.is_file():
        raise ArtifactError(f"Incomplete cached artifact in {image_path.parent}")

    try:
        with provenance_path.open(encoding="utf-8") as provenance_file:
            provenance = json.load(provenance_file)
    except (OSError, json.JSONDecodeError) as error:
        raise ArtifactError(f"Invalid cached provenance: {provenance_path}") from error

    expected = {
        "schemaVersion": 1,
        "build.id": build["id"],
        "build.definitionId": build["definition"]["id"],
        "build.result": build["result"],
        "build.sourceBranch": build["sourceBranch"],
        "build.sourceVersion": build["sourceVersion"],
        "artifact.id": artifact.get("id"),
        "artifact.name": artifact["name"],
        "artifact.type": artifact["resource"]["type"],
        "artifact.size": artifact_size(artifact),
        "image.filename": config["imageFilename"],
    }
    actual = {
        "schemaVersion": provenance.get("schemaVersion"),
        "build.id": provenance.get("build", {}).get("id"),
        "build.definitionId": provenance.get("build", {}).get("definitionId"),
        "build.result": provenance.get("build", {}).get("result"),
        "build.sourceBranch": provenance.get("build", {}).get("sourceBranch"),
        "build.sourceVersion": provenance.get("build", {}).get("sourceVersion"),
        "artifact.id": provenance.get("artifact", {}).get("id"),
        "artifact.name": provenance.get("artifact", {}).get("name"),
        "artifact.type": provenance.get("artifact", {}).get("type"),
        "artifact.size": provenance.get("artifact", {}).get("size"),
        "image.filename": provenance.get("image", {}).get("filename"),
    }
    mismatches = [
        f"{key}: expected {expected[key]!r}, got {actual[key]!r}"
        for key in expected
        if actual[key] != expected[key]
    ]
    if mismatches:
        raise ArtifactError("Cached provenance mismatch:\n" + "\n".join(mismatches))

    image_size = provenance.get("image", {}).get("size")
    image_sha256 = provenance.get("image", {}).get("sha256")
    if (
        not isinstance(image_size, int)
        or image_size <= 0
        or not isinstance(image_sha256, str)
        or len(image_sha256) != 64
        or any(character not in "0123456789abcdef" for character in image_sha256)
    ):
        raise ArtifactError(f"Invalid cached image provenance: {provenance_path}")
    if not image_matches(image_path, image_size, image_sha256):
        raise ArtifactError(f"Cached image does not match provenance: {image_path}")
    return image_size, image_sha256


def write_provenance(
    config: Dict[str, Any],
    build: Dict[str, Any],
    artifact: Dict[str, Any],
    image_path: Path,
    provenance_path: Path,
    cache_hit: bool,
    image_size: int,
    image_sha256: str,
) -> None:
    provenance = {
        "schemaVersion": 1,
        "verifiedAt": datetime.now(timezone.utc).isoformat(),
        "cacheHit": cache_hit,
        "build": {
            "id": build["id"],
            "number": build.get("buildNumber"),
            "definitionId": build["definition"]["id"],
            "status": build["status"],
            "result": build["result"],
            "sourceBranch": build["sourceBranch"],
            "sourceVersion": build["sourceVersion"],
        },
        "artifact": {
            "id": artifact.get("id"),
            "name": artifact["name"],
            "type": artifact["resource"]["type"],
            "size": artifact_size(artifact),
        },
        "image": {
            "path": str(image_path),
            "filename": config["imageFilename"],
            "size": image_size,
            "sha256": image_sha256,
        },
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


def prepare_artifact(
    config: Dict[str, Any],
    output_root: Path,
    link_path: Path,
    token: str,
    build_id: Optional[int] = None,
) -> Path:
    output_root = output_root.expanduser().resolve()
    link_path = link_path.expanduser().absolute()
    build = resolve_build(config, token, build_id)
    resolved_build_id = build["id"]
    artifact_query = urllib.parse.urlencode(
        {
            "artifactName": config["artifactName"],
            "api-version": "7.1",
        }
    )
    artifact_url = build_api_url(
        config, f"build/builds/{resolved_build_id}/artifacts?{artifact_query}"
    )

    artifact = request_json(artifact_url, token)
    download_url = verify_artifact(config, artifact)

    output_dir = output_root / f"build-{resolved_build_id}"
    output_dir.mkdir(parents=True, exist_ok=True)
    image_path = output_dir / config["imageFilename"]
    provenance_path = output_dir / "provenance.json"
    image_metadata = cached_image_metadata(
        config,
        build,
        artifact,
        image_path,
        provenance_path,
    )
    cache_hit = image_metadata is not None

    if not cache_hit:
        with tempfile.NamedTemporaryFile(
            dir=output_dir, prefix="artifact-", suffix=".zip", delete=False
        ) as temporary_file:
            archive_path = Path(temporary_file.name)
        try:
            download_file(download_url, token, archive_path)
            image_metadata = extract_image(config, archive_path, image_path)
        finally:
            archive_path.unlink(missing_ok=True)

    if image_metadata is None:
        raise ArtifactError("Functional-test image metadata was not resolved")
    image_size, image_sha256 = image_metadata
    write_provenance(
        config,
        build,
        artifact,
        image_path,
        provenance_path,
        cache_hit,
        image_size,
        image_sha256,
    )
    ensure_link(image_path, link_path)
    return provenance_path


def positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("must be a positive integer") from error
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def parse_args() -> argparse.Namespace:
    script_dir = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(
        description=(
            "Download the latest successful Trident functional-test image and "
            "record its immutable provenance."
        )
    )
    parser.add_argument(
        "--config",
        type=Path,
        default=script_dir / "functional-artifact.json",
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=Path(
            os.environ.get(
                "TRIDENT_FUNCTIONAL_CACHE_DIR",
                "/mnt/storage/trident-cloud-agent/functional",
            )
        ),
    )
    parser.add_argument(
        "--link-path",
        type=Path,
        default=script_dir.parent.parent / "artifacts" / "trident-functest.qcow2",
    )
    build_id = os.environ.get("TRIDENT_FUNCTIONAL_BUILD_ID")
    parser.add_argument(
        "--build-id",
        type=positive_int,
        default=positive_int(build_id) if build_id else None,
        help=(
            "Use a specific successful trident-ci build instead of resolving the "
            "latest build (default: TRIDENT_FUNCTIONAL_BUILD_ID)."
        ),
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    config = load_config(args.config)
    provenance_path = prepare_artifact(
        config,
        args.output_root,
        args.link_path,
        get_ado_token(),
        args.build_id,
    )
    print(provenance_path)


if __name__ == "__main__":
    main()
