#!/usr/bin/env python3
"""Read-only qualification of the existing 1ES caller's ADO metadata access."""

import argparse
import json
import shutil
import signal
import sys
import urllib.error

from pathlib import Path

import download_functional_artifact as ado
from prepare_vm_e2e_artifacts import PROJECT_ID, STAGES

BUILD_ID = 1266405
DEFINITION_ID = 5073
DEFINITION_REVISION = 7
REVISION = "18dd6cd99d50a4268afd936acb1ee4d2b64881ea"
REF = "refs/heads/user/britel/cloud-vm-e2e-install"
ARTIFACTS = {
    "go-tools": (5617669, 179833188),
    "trident-installer": (5617787, 262809600),
    "trident-split-installer": (5617785, 234829824),
    "trident-testimage": (5617779, 355480064),
    "trident-verity-testimage": (5617776, 348007424),
    "trident-usrverity-testimage": (5617792, 425632567),
}
SKIPPED = frozenset(
    """
    DefineTests_E2E E2ETesting_VM_HOST E2ETesting_VM_CONTAINER
    BuildTridentContainerImage TridentTestImg_trident_container_installer
    TridentTestImg_trident_container_testimage
    TridentTestImg_trident_container_verity_testimage
    TridentTestImg_trident_container_usrverity_testimage
    BuildInstallerTools AzureLinux_Installer_ISO
    Test_AzureLinux_Installer_ISO_Unattended RuntimeUpdateAndRollbackTesting
    PrepareSSHKeys BuildImagesQemu BuildImagesUKI ServicingTesting
    BuildAclImages TestAcl BuildImagesAclAgent TridentAclAgentTest FunctionalTesting
    DefineTests_VM_host DeploymentTesting_host DefineTests_VM_container
    DeploymentTesting_container BuildInstallerForDirectStreaming_amd64
    BuildImagesForDirectStreaming_azurelinux_amd64
    DirectStreaming_DeploymentTesting_azurelinux_direct_streaming_testimage_amd64
    BuildImagesForDirectStreaming_ubuntu_2204_amd64
    DirectStreaming_DeploymentTesting_ubuntu_direct_streaming_testimage_2204_amd64
    BuildImagesForDirectStreaming_ubuntu_2404_amd64
    DirectStreaming_DeploymentTesting_ubuntu_direct_streaming_testimage_2404_amd64
    GetTridentBinaries_rpms_arm64 BuildingTools_arm64
    BuildInstallerForDirectStreaming_arm64
    BuildImagesForDirectStreaming_azurelinux_arm64
    DirectStreaming_DeploymentTesting_azurelinux_direct_streaming_testimage_arm64
    BuildImagesForDirectStreaming_ubuntu_2204_arm64
    DirectStreaming_DeploymentTesting_ubuntu_direct_streaming_testimage_2204_arm64
    BuildImagesForDirectStreaming_ubuntu_2404_arm64
    DirectStreaming_DeploymentTesting_ubuntu_direct_streaming_testimage_2404_arm64
    """.split()
)
CONFIG = {
    "organization": "mariner-org",
    "project": PROJECT_ID,
    "definitionId": DEFINITION_ID,
    "sourceBranch": REF,
    "buildStatus": "completed",
    "buildResult": "succeeded",
}


def verify_timeline(timeline):
    rows = [r for r in timeline["records"] if r.get("type") == "Stage"]
    expected = set(STAGES.values()) | SKIPPED
    if (
        len(rows) != 48
        or {r["identifier"] for r in rows} != expected
        or len(SKIPPED) != 41
    ):
        raise ValueError("Stage inventory mismatch")
    for row in rows:
        result = "skipped" if row["identifier"] in SKIPPED else "succeeded"
        if row.get("state") != "completed" or row.get("result") != result:
            raise ValueError("Stage partition mismatch")


def diagnostic(error, check):
    # Never serialize exceptions, CLI stderr, URLs, response bodies or tokens.
    cause = error
    while cause is not None:
        if isinstance(cause, urllib.error.HTTPError):
            status = cause.code
            code = {
                401: "ado_authentication_rejected",
                403: "ado_access_denied",
                404: "ado_resource_unavailable",
            }.get(status, "ado_http_failure")
            return {"code": code, "httpStatus": status}
        cause = cause.__cause__
    if isinstance(error, TimeoutError):
        return {"code": "probe_deadline_exceeded"}
    if check == "existingCredential":
        return {"code": "prerequisite_credential_not_initialized_or_unavailable"}
    if isinstance(error, ado.ArtifactError):
        return {"code": "ado_read_or_metadata_validation_failed"}
    return {"code": "probe_validation_or_execution_failed"}


def probe(output):
    output.mkdir(parents=True, exist_ok=False)
    report = {
        "schemaVersion": 1,
        "scope": "existing-caller-ado-metadata-access-only",
        "expected": {
            "organization": CONFIG["organization"],
            "projectId": PROJECT_ID,
            "buildId": BUILD_ID,
            "definitionId": DEFINITION_ID,
            "definitionRevision": DEFINITION_REVISION,
            "sourceVersion": REVISION,
            "sourceBranch": REF,
        },
        "outcome": "failed",
        "artifactContentAccess": "not_tested",
        "credentialPrerequisite": "not_tested",
        "credentialInitializationPerformedByProbe": False,
        "adoAuthorization": "not_tested",
        "provenanceQualified": False,
        "vmExecuted": False,
        "checks": [],
        "artifacts": [],
    }

    def save():
        (output / "summary.json").write_text(
            json.dumps(report, indent=2) + "\n", encoding="utf-8"
        )

    def start(name):
        report["checks"].append({"name": name, "outcome": "started"})
        save()

    def passed():
        report["checks"][-1]["outcome"] = "passed"
        save()

    save()
    try:
        start("azureCliAvailable")
        if not (shutil.which("az") or shutil.which("az.cmd")):
            report["checks"][-1]["diagnostic"] = {"code": "azure_cli_unavailable"}
            raise FileNotFoundError()
        passed()
        start("existingCredential")
        token = ado.get_ado_token()
        if not token:
            raise ValueError("Empty credential")
        report["credentialPrerequisite"] = "usable"
        passed()
        start("producerIdentity")
        report["adoAuthorization"] = "incomplete"
        build = ado.resolve_build(CONFIG, token, BUILD_ID)
        if (
            build.get("project", {}).get("id") != PROJECT_ID
            or build["sourceVersion"] != REVISION
            or build["definition"].get("revision") != DEFINITION_REVISION
        ):
            raise ValueError("Producer identity mismatch")
        passed()
        start("stagePartition")
        base = f"build/builds/{BUILD_ID}"
        verify_timeline(
            ado.request_json(
                ado.build_api_url(CONFIG, f"{base}/timeline?api-version=7.1"), token
            )
        )
        report["stages"] = {
            "succeeded": sorted(STAGES.values()),
            "skipped": sorted(SKIPPED),
        }
        passed()
        for name, (artifact_id, size) in ARTIFACTS.items():
            start(f"artifactMetadata:{name}")
            artifact = ado.request_json(
                ado.build_api_url(
                    CONFIG, f"{base}/artifacts?artifactName={name}&api-version=7.1"
                ),
                token,
            )
            ado.verify_artifact(
                {"artifactName": name, "artifactType": "PipelineArtifact"}, artifact
            )
            if artifact.get("id") != artifact_id or ado.artifact_size(artifact) != size:
                raise ValueError("Artifact identity mismatch")
            report["artifacts"].append(
                {
                    "name": name,
                    "id": artifact_id,
                    "size": size,
                    "type": "PipelineArtifact",
                }
            )
            passed()
        report["outcome"] = "passed"
        report["adoAuthorization"] = "selected_metadata_reads_confirmed"
        return 0
    except Exception as error:
        check = report["checks"][-1]
        check["outcome"] = "failed"
        check.setdefault("diagnostic", diagnostic(error, check["name"]))
        if check["name"] == "existingCredential":
            report["credentialPrerequisite"] = "not_initialized_or_unavailable"
        if check["diagnostic"].get("httpStatus") == 403:
            report["adoAuthorization"] = "denied"
        elif check["diagnostic"].get("httpStatus") == 401:
            report["adoAuthorization"] = "authentication_rejected"
        return 1
    finally:
        save()


def deadline(signum, frame):
    raise TimeoutError()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, deadline)
    signal.signal(signal.SIGALRM, deadline)
    signal.alarm(360)
    try:
        return probe(args.output)
    except Exception:
        print("Access probe could not write its bounded report.", file=sys.stderr)
        return 1
    finally:
        signal.alarm(0)


if __name__ == "__main__":
    sys.exit(main())
