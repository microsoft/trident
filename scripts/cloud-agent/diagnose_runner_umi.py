#!/usr/bin/env python3

"""One bounded UMI login observation, never publish native output or credentials."""

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile

from pathlib import Path

import inspect_vm_e2e_images as inspection
import vm_e2e_failure_facts as facts

CLIENT_ID = "e2fd61e6-b160-4f02-a5ac-3d949a4fe01d"
LOGIN = [
    "az",
    "login",
    "--identity",
    "--client-id",
    CLIENT_ID,
    "--allow-no-subscriptions",
    "--output",
    "none",
]
PRIVATE_LIMIT = 64 * 1024
REPORT_LIMIT = 8192
ENVIRONMENT = (
    "AZURE_CONFIG_DIR",
    "AZURE_EXTENSION_DIR",
    "AZURE_CLIENT_ID",
    "AZURE_TENANT_ID",
    "AZURE_AUTHORITY_HOST",
    "AZURE_CLIENT_SECRET",
    "AZURE_FEDERATED_TOKEN_FILE",
    "AZURE_ACCESS_TOKEN",
    "AZURE_DEVOPS_EXT_PAT",
    "SYSTEM_ACCESSTOKEN",
    "MSI_ENDPOINT",
    "MSI_SECRET",
    "IDENTITY_ENDPOINT",
    "IDENTITY_HEADER",
    "IDENTITY_SERVER_THUMBPRINT",
    "IMDS_ENDPOINT",
    "AZURE_POD_IDENTITY_AUTHORITY_HOST",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
)
ARGUMENTS = ("identity", "client-id", "allow-no-subscriptions", "output")
ERROR_CODES = {
    "invalid_request": "provider-invalid-request",
    "invalid_client": "provider-invalid-client",
    "unauthorized_client": "provider-unauthorized-client",
    "invalid_resource": "provider-invalid-resource",
    "invalid_scope": "provider-invalid-scope",
    "invalid_tenant": "provider-invalid-tenant",
    "AuthorizationFailed": "arm-authorization-failed",
    "AuthenticationFailed": "authentication-failed",
    "IdentityNotFound": "identity-not-found",
    "InvalidAuthenticationTokenTenant": "token-issuer-mismatch",
    "InvalidAuthenticationToken": "invalid-authentication-token",
}
MARKERS = {
    "unsupported-login-argument": r"unrecognized arguments:\s*--(?:client-id|identity|allow-no-subscriptions|output)\b",
    "identity-not-found": r"(?:\([a-z_]+\)\s*)?(?:Identity not found|No MSI found for specified ClientId/ResourceId/PrincipalId)(?:[.\r\n]|$)",
    "identity-not-assigned": r"(?:ManagedIdentityCredential authentication failed:\s*)?The requested identity has not been assigned to this resource\b",
    "msi-connection-failed": r"Failed to connect to MSI\b",
    "imds-token-request-failed": r"MSI: Failed to retrieve a token from IMDS endpoint\b",
    "proxy-error": r"(?:requests\.exceptions\.)?ProxyError:",
    "tls-error": r"(?:requests\.exceptions\.)?SSLError:",
    "connection-error": r"(?:requests\.exceptions\.)?(?:ConnectionError|ConnectTimeout|ReadTimeout):",
}
AADSTS = {
    "700016": "entra-application-not-found",
    "90002": "entra-tenant-not-found",
    "500011": "entra-resource-not-found",
}


def new_report():
    return {
        "schemaVersion": 1,
        "scope": "runner-umi-login-only-not-ado-or-guest-execution",
        "basis": "literal-cli-observations-not-proof-of-attachment-or-root-cause",
        "status": "not-started",
        "activeOperation": "not-started",
        "operationBasis": "last-started-not-proof-of-cause-or-completion",
        "requestedClientId": CLIENT_ID,
        "context": "fresh-private-root-profile-no-ambient-profile-comparison",
        "environmentPresence": None,
        "preflight": None,
        "cliVersion": None,
        "argumentSupport": None,
        "commands": {
            name: {"state": "not-run", "exitCode": None, "exceptionClass": None}
            for name in ("version", "help", "login")
        },
        "loginFailureMarkers": [],
        "reportedHttpStatus": None,
        "publicationFailure": None,
        "exceptionClass": None,
        "hostDisposition": "dispose-required",
    }


def publish(output, report):
    inspection.require(
        len((json.dumps(report, indent=2) + "\n").encode()) < REPORT_LIMIT,
        "diagnostic-report-limit",
    )
    inspection.write_report(output, report)


def failed_publication(output, missing_fact, report=None, *, error=None):
    report = new_report() if report is None else report
    report["status"] = "failed"
    report["publicationFailure"] = {
        "privileged-inspection-not-started": "not-started",
        "privileged-inspection-cancelled": "cancelled-context",
        "privileged-inspection-setup-failed": "private-context-failed",
        "exact-report-publication-failed": "publication-finalize-failed",
    }.get(missing_fact, "unavailable")
    publish(output, report)


def capture(argv, timeout):
    result = {"state": "failed", "exitCode": None, "exceptionClass": None}
    with (
        tempfile.TemporaryFile(dir=os.environ["HOME"]) as stdout,
        tempfile.TemporaryFile(dir=os.environ["HOME"]) as stderr,
    ):
        try:
            process = subprocess.Popen(
                argv,
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
                env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
            )
        except OSError as error:
            name = type(error).__name__
            result["exceptionClass"] = name if name in facts.EXCEPTIONS else None
            return result, None, None
        try:
            code = process.wait(timeout=timeout)
            inspection.VM_RUNNER.signal_group(process, signal.SIGKILL)
            result.update(state="ok" if code == 0 else "failed")
        except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
            code = 130 if isinstance(error, KeyboardInterrupt) else 124
            result.update(
                state="cancelled" if code == 130 else "timeout",
                exceptionClass=type(error).__name__,
            )
            inspection.VM_RUNNER.reap(process, 10, group=True)
        except BaseException:
            inspection.VM_RUNNER.reap(process, 10, group=True)
            raise
        result["exitCode"] = code if -128 <= code <= 255 else None
        samples = []
        for stream in (stdout, stderr):
            stream.seek(0)
            raw = stream.read(PRIVATE_LIMIT + 1)
            samples.append(raw if len(raw) <= PRIVATE_LIMIT else None)
        return result, *samples


def login_observations(raw):
    if raw is None or len(raw) > PRIVATE_LIMIT:
        return ["unclassified"], None
    text = raw.decode("utf-8", errors="replace")
    markers = set()
    statuses = set()
    for line in text.splitlines():
        line = re.sub(r"^ERROR:\s*", "", line)
        for label, pattern in MARKERS.items():
            if re.match(pattern, line):
                markers.add(label)
        code = re.match(r"^(?:Code:\s*|\()([A-Za-z_]+)(?:\)|\s*$)", line)
        if code and code[1] in ERROR_CODES:
            markers.add(ERROR_CODES[code[1]])
        aad = re.match(r"^AADSTS([0-9]+):", line)
        if aad and aad[1] in AADSTS:
            markers.add(AADSTS[aad[1]])
        status = re.match(
            r"^(?:(?:Status Code|Status code|HTTP status|Response status):\s*"
            r"|Get Token request returned http error:\s*"
            r"|(?:requests\.exceptions\.)?HTTPError:\s*)([0-9]{3})(?:\s|,|$)",
            line,
        )
        if status and int(status[1]) in facts.HTTP_STATUSES:
            statuses.add(int(status[1]))
        if line.startswith("{"):
            try:
                body = json.loads(line)
            except (ValueError, RecursionError):
                continue
            if isinstance(body, dict):
                error = body.get("error")
                if isinstance(error, dict):
                    error = error.get("code")
                if isinstance(error, str) and error in ERROR_CODES:
                    markers.add(ERROR_CODES[error])
                description = body.get("error_description")
                if isinstance(description, str):
                    for label, pattern in MARKERS.items():
                        if re.match(pattern, description):
                            markers.add(label)
                    aad = re.match(r"^AADSTS([0-9]+):", description)
                    if aad and aad[1] in AADSTS:
                        markers.add(AADSTS[aad[1]])
    return sorted(markers) or ["unclassified"], (
        next(iter(statuses)) if len(statuses) == 1 else None
    )


def execute(output, scratch, report):
    code = 1
    handlers = {}
    try:
        for sig in (signal.SIGINT, signal.SIGTERM):
            handlers[sig] = signal.signal(sig, inspection.VM_RUNNER.interrupt)
        report["status"] = "failed"
        report["activeOperation"] = "preflight"
        publish(output, report)
        report["preflight"] = {
            "linux": sys.platform == "linux",
            "root": os.geteuid() == 0,
            "exclusiveDisposable": os.environ.get("EXCLUSIVE_DISPOSABLE_1ES") == "true",
            "azureCli": bool(shutil.which("az")),
        }
        inspection.require(
            all(report["preflight"].values()),
            "exclusive-disposable-root-context-required",
        )
        report["environmentPresence"] = {
            name: bool(os.environ.get(name)) for name in ENVIRONMENT
        }
        for name, argv, timeout in (
            ("version", ["az", "version", "--output", "json"], 30),
            ("help", ["az", "login", "--help"], 30),
            ("login", LOGIN, 120),
        ):
            report["activeOperation"] = name
            publish(output, report)
            row, stdout, stderr = capture(argv, timeout)
            report["commands"][name] = row
            if name == "version" and row["state"] == "ok" and stdout is not None:
                try:
                    version = json.loads(stdout).get("azure-cli")
                    if isinstance(version, str) and re.fullmatch(
                        r"[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}", version
                    ):
                        report["cliVersion"] = version
                except (ValueError, AttributeError, RecursionError):
                    report["cliVersion"] = None
            if name == "help" and row["state"] == "ok" and stdout is not None:
                report["argumentSupport"] = {
                    flag: bool(
                        re.search(
                            rb"(?m)^[ \t]*--" + flag.encode() + rb"(?:[ \t,:=]|$)",
                            stdout,
                        )
                    )
                    for flag in ARGUMENTS
                }
            if row["state"] in ("timeout", "cancelled"):
                code = row["exitCode"]
                report["status"] = row["state"]
                break
            if row["exitCode"] is None:
                break
            if name == "login":
                if row["state"] == "ok":
                    report["status"] = "login-succeeded-not-guest-execution"
                    code = 0
                else:
                    report["loginFailureMarkers"], report["reportedHttpStatus"] = (
                        login_observations(stderr)
                    )
            publish(output, report)
    except KeyboardInterrupt:
        report["status"] = "cancelled"
        code = 130
    except Exception as error:
        report["status"] = "failed"
        code = code or 1
        name = type(error).__name__
        report["exceptionClass"] = name if name in facts.EXCEPTIONS else None
    finally:
        try:
            with inspection.VM_RUNNER.cleanup_signals():
                publish(output, report)
        finally:
            for sig, handler in handlers.items():
                signal.signal(sig, handler)
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--prepare-publication", action="store_true")
    parser.add_argument("--publication-uid", type=int)
    parser.add_argument("--publication-gid", type=int)
    args = parser.parse_args()
    os.umask(0o077)
    report = new_report()
    fail = lambda output, fact, **kwargs: failed_publication(
        output, fact, report, **kwargs
    )
    if args.prepare_publication:
        inspection.prepare_publication(
            args.output, publication_name="diagnostic.json", failure_fn=fail
        )
        return 0

    def cancel(signum, frame):
        raise inspection.Cancelled()

    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, cancel)
    try:
        return inspection.privileged_inspection(
            args,
            publication_name="diagnostic.json",
            failure_fn=fail,
            execute_fn=lambda output, scratch, bootstrap_guestfish: execute(
                output, scratch, report
            ),
            retain_secondary_failure=True,
        )
    except inspection.Cancelled:
        return 130
    except (Exception, KeyboardInterrupt):
        return 1


if __name__ == "__main__":
    sys.exit(main())
