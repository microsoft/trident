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
from typing import Any, Dict

ADO_RESOURCE = "499b84ac-1321-427f-aa17-267ca6975798"
CHUNK_SIZE = 1024 * 1024


class ArtifactError(RuntimeError):
    pass


def load_config(path: Path) -> Dict[str, Any]:
    with path.open(encoding="utf-8") as config_file:
        config = json.load(config_file)
    if config.get("schemaVersion") != 1:
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
        "id": config["buildId"],
        "definition.id": config["definitionId"],
        "result": config["buildResult"],
        "sourceBranch": config["sourceBranch"],
        "sourceVersion": config["sourceVersion"],
    }
    actual = {
        "id": build.get("id"),
        "definition.id": build.get("definition", {}).get("id"),
        "result": build.get("result"),
        "sourceBranch": build.get("sourceBranch"),
        "sourceVersion": build.get("sourceVersion"),
    }
    mismatches = [
        f"{key}: expected {expected[key]!r}, got {actual[key]!r}"
        for key in expected
        if actual[key] != expected[key]
    ]
    if mismatches:
        raise ArtifactError("Pinned build metadata mismatch:\n" + "\n".join(mismatches))


def verify_artifact(config: Dict[str, Any], artifact: Dict[str, Any]) -> str:
    resource = artifact.get("resource", {})
    properties = resource.get("properties", {})
    expected = {
        "name": config["artifactName"],
        "resource.type": config["artifactType"],
        "resource.properties.artifactsize": str(config["artifactSize"]),
    }
    actual = {
        "name": artifact.get("name"),
        "resource.type": resource.get("type"),
        "resource.properties.artifactsize": properties.get("artifactsize"),
    }
    mismatches = [
        f"{key}: expected {expected[key]!r}, got {actual[key]!r}"
        for key in expected
        if actual[key] != expected[key]
    ]
    if mismatches:
        raise ArtifactError(
            "Pinned artifact metadata mismatch:\n" + "\n".join(mismatches)
        )

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


def image_matches(config: Dict[str, Any], path: Path) -> bool:
    return (
        path.is_file()
        and path.stat().st_size == config["imageSize"]
        and hash_file(path) == config["imageSha256"]
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


def extract_image(config: Dict[str, Any], archive_path: Path, image_path: Path) -> None:
    temporary_path = image_path.with_suffix(f"{image_path.suffix}.tmp")
    temporary_path.unlink(missing_ok=True)
    try:
        with zipfile.ZipFile(archive_path) as archive:
            entry = find_image_entry(archive, config["imageFilename"])
            if entry.file_size != config["imageSize"]:
                raise ArtifactError(
                    f"Image size mismatch: expected {config['imageSize']}, "
                    f"got {entry.file_size}"
                )

            digest = hashlib.sha256()
            with archive.open(entry) as source, temporary_path.open("wb") as output:
                while chunk := source.read(CHUNK_SIZE):
                    output.write(chunk)
                    digest.update(chunk)

        actual_hash = digest.hexdigest()
        if actual_hash != config["imageSha256"]:
            raise ArtifactError(
                f"Image SHA-256 mismatch: expected {config['imageSha256']}, "
                f"got {actual_hash}"
            )
        os.replace(temporary_path, image_path)
    finally:
        temporary_path.unlink(missing_ok=True)


def ensure_link(image_path: Path, link_path: Path) -> None:
    link_path.parent.mkdir(parents=True, exist_ok=True)
    if link_path.is_symlink():
        if link_path.resolve() == image_path.resolve():
            return
        raise ArtifactError(f"Refusing to replace existing symlink: {link_path}")
    if link_path.exists():
        raise ArtifactError(f"Refusing to replace existing path: {link_path}")
    link_path.symlink_to(image_path)


def write_provenance(
    config: Dict[str, Any],
    build: Dict[str, Any],
    artifact: Dict[str, Any],
    image_path: Path,
    provenance_path: Path,
    cache_hit: bool,
) -> None:
    provenance = {
        "schemaVersion": 1,
        "verifiedAt": datetime.now(timezone.utc).isoformat(),
        "cacheHit": cache_hit,
        "build": {
            "id": build["id"],
            "number": build.get("buildNumber"),
            "definitionId": build["definition"]["id"],
            "result": build["result"],
            "sourceBranch": build["sourceBranch"],
            "sourceVersion": build["sourceVersion"],
        },
        "artifact": {
            "id": artifact.get("id"),
            "name": artifact["name"],
            "type": artifact["resource"]["type"],
            "size": int(artifact["resource"]["properties"]["artifactsize"]),
        },
        "image": {
            "path": str(image_path),
            "filename": config["imageFilename"],
            "size": image_path.stat().st_size,
            "sha256": config["imageSha256"],
        },
    }
    temporary_path = provenance_path.with_suffix(".tmp")
    with temporary_path.open("w", encoding="utf-8") as provenance_file:
        json.dump(provenance, provenance_file, indent=2)
        provenance_file.write("\n")
    os.replace(temporary_path, provenance_path)


def prepare_artifact(
    config: Dict[str, Any],
    output_root: Path,
    link_path: Path,
    token: str,
) -> Path:
    output_root = output_root.expanduser().resolve()
    link_path = link_path.expanduser().absolute()
    build_id = config["buildId"]
    build_url = build_api_url(config, f"build/builds/{build_id}?api-version=7.1")
    artifact_query = urllib.parse.urlencode(
        {
            "artifactName": config["artifactName"],
            "api-version": "7.1",
        }
    )
    artifact_url = build_api_url(
        config, f"build/builds/{build_id}/artifacts?{artifact_query}"
    )

    build = request_json(build_url, token)
    verify_build(config, build)
    artifact = request_json(artifact_url, token)
    download_url = verify_artifact(config, artifact)

    output_dir = output_root / f"build-{build_id}"
    output_dir.mkdir(parents=True, exist_ok=True)
    image_path = output_dir / config["imageFilename"]
    provenance_path = output_dir / "provenance.json"
    cache_hit = image_matches(config, image_path)

    if not cache_hit:
        if image_path.exists():
            raise ArtifactError(
                f"Cached image does not match pinned provenance: {image_path}"
            )
        with tempfile.NamedTemporaryFile(
            dir=output_dir, prefix="artifact-", suffix=".zip", delete=False
        ) as temporary_file:
            archive_path = Path(temporary_file.name)
        try:
            download_file(download_url, token, archive_path)
            extract_image(config, archive_path, image_path)
        finally:
            archive_path.unlink(missing_ok=True)

    ensure_link(image_path, link_path)
    write_provenance(
        config,
        build,
        artifact,
        image_path,
        provenance_path,
        cache_hit,
    )
    return provenance_path


def parse_args() -> argparse.Namespace:
    script_dir = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(
        description="Download and verify the pinned Trident functional-test image."
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
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    config = load_config(args.config)
    provenance_path = prepare_artifact(
        config,
        args.output_root,
        args.link_path,
        get_ado_token(),
    )
    print(provenance_path)


if __name__ == "__main__":
    main()
