#!/usr/bin/env python3

"""
Download release artifacts from an Azure DevOps pipeline run, validate their
SHA-256 checksums, and upload them to a GitHub release.

Prerequisites:
    - `az` CLI authenticated with access to the mariner-org ADO organization
    - `gh` CLI authenticated with access to the target GitHub repository

Usage:
    python3 scripts/release-artifacts.py \\
        --run-id 123456 \\
        --tag v1.0.0 \\
        --repo microsoft/trident
"""

import argparse
import hashlib
import logging
import os
import subprocess
import sys
import tempfile
from pathlib import Path

logging.basicConfig(
    level=logging.INFO,
    format="%(levelname)s: %(message)s",
)
log = logging.getLogger(__name__)

DEFAULT_ADO_ORG = "https://dev.azure.com/mariner-org"
DEFAULT_ADO_PROJECT = "ECF"
DEFAULT_ARTIFACT_NAME = "release-artifacts"
CHECKSUM_EXT = ".sha256"


def download_artifact(
    run_id: str,
    dest: Path,
    org: str,
    project: str,
    artifact_name: str,
) -> None:
    """Download the release-artifacts artifact from the given pipeline run."""
    log.info("Downloading artifact '%s' from run %s …", artifact_name, run_id)
    cmd = [
        "az",
        "pipelines",
        "runs",
        "artifact",
        "download",
        "--org",
        org,
        "--project",
        project,
        "--run-id",
        run_id,
        "--artifact-name",
        artifact_name,
        "--path",
        str(dest),
    ]
    result = subprocess.run(cmd, capture_output=True)
    if result.returncode != 0:
        log.error("Failed to download artifact: %s", result.stderr.decode())
        sys.exit(1)
    log.info("Artifacts downloaded to %s", dest)


def _human_size(size: int) -> str:
    """Return a human-readable file size string."""
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if abs(size) < 1024:
            return f"{size:.1f} {unit}"
        size /= 1024  # type: ignore[assignment]
    return f"{size:.1f} PiB"


def validate_checksums(
    artifact_dir: Path,
    accept_no_checksum: bool = False,
) -> list[Path]:
    """Validate SHA-256 checksums and return the list of release files.

    For every file that has a corresponding .sha256 sidecar, the checksum is
    verified.  All non-checksum files are returned (regardless of whether they
    had a sidecar) so they can be uploaded to the release.

    Unless *accept_no_checksum* is ``True``, files without a matching .sha256
    sidecar are treated as errors.
    """
    all_files = sorted(f for f in artifact_dir.iterdir() if f.is_file())
    checksum_files = {f for f in all_files if f.name.endswith(CHECKSUM_EXT)}
    release_files: list[Path] = []
    errors: list[str] = []

    for f in all_files:
        if f.name.endswith(CHECKSUM_EXT):
            continue

        release_files.append(f)

        sha_file = artifact_dir / (f.name + CHECKSUM_EXT)
        if sha_file not in checksum_files:
            if accept_no_checksum:
                log.warning("No checksum file for %s — skipping verification", f.name)
            else:
                msg = f"No checksum file for {f.name}"
                log.error(msg)
                errors.append(msg)
            continue

        # The .sha256 file may be produced by `sha256sum` with the format:
        #   <hex_digest>  <filename>
        # Strip everything after the hash so the uploaded sidecar contains
        # only the hex digest.
        expected_line = sha_file.read_text().strip()
        expected_hash = expected_line.split()[0].lower()

        sha = hashlib.sha256()
        with open(f, "rb") as fh:
            for chunk in iter(lambda: fh.read(1 << 20), b""):
                sha.update(chunk)
        actual_hash = sha.hexdigest()

        if actual_hash != expected_hash:
            msg = f"Checksum mismatch for {f.name}: expected {expected_hash}, got {actual_hash}"
            log.error(msg)
            errors.append(msg)
        else:
            log.info("Checksum OK: %s", f.name)

        # Rewrite the sidecar to contain only the hex digest
        sha_file.write_text(expected_hash + "\n")

        # Include the .sha256 file in the release as well
        release_files.append(sha_file)

    if errors:
        log.error("Checksum validation failed — aborting upload")
        sys.exit(1)

    return release_files


def upload_to_release(
    files: list[Path],
    tag: str,
    repo: str,
    overwrite: bool = False,
) -> None:
    """Upload files to a GitHub release identified by *tag*."""
    log.info("Uploading %d file(s) to release %s on %s …", len(files), tag, repo)
    cmd = [
        "gh",
        "release",
        "upload",
        tag,
        "--repo",
        repo,
    ]
    if overwrite:
        cmd.append("--clobber")
    cmd.extend(str(f) for f in files)

    result = subprocess.run(cmd, capture_output=True)
    if result.returncode != 0:
        log.error("Failed to upload to release: %s", result.stderr.decode())
        sys.exit(1)
    log.info("Upload complete")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Download ADO pipeline release artifacts, verify checksums, and upload to a GitHub release.",
    )
    parser.add_argument(
        "-r",
        "--run-id",
        required=True,
        help="Azure DevOps pipeline run ID to download artifacts from.",
    )
    parser.add_argument(
        "-t",
        "--tag",
        required=True,
        help="GitHub release tag to upload assets to (e.g. v1.0.0).",
    )
    parser.add_argument(
        "--repo",
        default="microsoft/trident",
        help="GitHub repository in OWNER/REPO format (default: microsoft/trident).",
    )
    parser.add_argument(
        "--org",
        default=DEFAULT_ADO_ORG,
        help=f"Azure DevOps organization URL (default: {DEFAULT_ADO_ORG}).",
    )
    parser.add_argument(
        "--project",
        default=DEFAULT_ADO_PROJECT,
        help=f"Azure DevOps project (default: {DEFAULT_ADO_PROJECT}).",
    )
    parser.add_argument(
        "--artifact-name",
        default=DEFAULT_ARTIFACT_NAME,
        help=f"Pipeline artifact name to download (default: {DEFAULT_ARTIFACT_NAME}).",
    )
    parser.add_argument(
        "--output-dir",
        default=None,
        help="Directory to download artifacts into. A temp directory is used if omitted.",
    )
    parser.add_argument(
        "--skip-upload",
        "--dry-run",
        action="store_true",
        help="Download and validate only — do not upload to GitHub.",
    )
    parser.add_argument(
        "--accept-no-checksum",
        action="store_true",
        help="Allow files without a .sha256 sidecar instead of treating them as errors.",
    )
    parser.add_argument(
        "--overwrite",
        action="store_true",
        help="Overwrite existing assets on the GitHub release.",
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Enable debug logging.",
    )
    args = parser.parse_args()

    if args.debug:
        logging.getLogger().setLevel(logging.DEBUG)

    def run(artifact_dir: Path) -> None:
        download_artifact(
            args.run_id,
            artifact_dir,
            org=args.org,
            project=args.project,
            artifact_name=args.artifact_name,
        )
        release_files = validate_checksums(
            artifact_dir,
            accept_no_checksum=args.accept_no_checksum,
        )

        log.info("Files to upload:")
        for f in release_files:
            log.info("  %-50s %s", f.name, _human_size(f.stat().st_size))

        if not args.skip_upload:
            upload_to_release(release_files, args.tag, args.repo, args.overwrite)

    if args.output_dir:
        artifact_dir = Path(args.output_dir)
        artifact_dir.mkdir(parents=True, exist_ok=True)
        run(artifact_dir)
    else:
        with tempfile.TemporaryDirectory(prefix="trident-release-") as tmp:
            run(Path(tmp))


if __name__ == "__main__":
    main()
