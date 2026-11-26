#!/usr/bin/env python3

"""One bounded UMI login observation, never publish native output or credentials."""

import argparse
import ast
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
    "--debug",
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
    "50020": "entra-tenant-user-mismatch",
    "53003": "entra-conditional-access-blocked",
    "7000215": "entra-invalid-client-secret",
    "7000222": "entra-expired-client-secret",
    "700024": "entra-assertion-time-invalid",
}
EXCEPTION_MARKERS = {
    **dict.fromkeys(("ImportError", "ModuleNotFoundError"), "dependency-import-error"),
    **dict.fromkeys(("DecodeError", "InvalidTokenError"), "decode-error"),
    "JSONDecodeError": "json-decode-error",
    **dict.fromkeys(("PermissionError", "FileNotFoundError", "OSError"), "os-io-error"),
    **dict.fromkeys(
        (
            "ConnectionError",
            "ConnectTimeout",
            "ReadTimeout",
            "MaxRetryError",
            "NewConnectionError",
            "NameResolutionError",
            "ServiceRequestError",
            "ServiceResponseError",
            "AzureConnectionError",
        ),
        "connection-error",
    ),
    "ProxyError": "proxy-error",
    "SSLError": "tls-error",
    **dict.fromkeys(
        ("ManagedIdentityError", "ClientAuthenticationError", "AuthenticationError"),
        "authentication-error",
    ),
    "HttpResponseError": "http-response-error",
    "HTTPError": "http-response-error",
    "CLIError": "cli-error",
    **dict.fromkeys(
        ("AttributeError", "TypeError", "KeyError", "ValueError"),
        "python-runtime-error",
    ),
}
LOGGERS = (
    "cli.azure.cli.core.auth.util",
    "azure.cli.core.auth.util",
    "cli.azure.cli.core._profile",
    "azure.cli.core._profile",
    "cli.azure.cli.core.azclierror",
    "azure.cli.core.azclierror",
    "msal.managed_identity",
    "urllib3.connectionpool",
    "azure.core.pipeline.policies.http_logging_policy",
)


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
        "httpStatusConflict": False,
        "loginOutput": None,
        "loginEvidence": None,
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
                env={
                    **os.environ,
                    "PYTHONDONTWRITEBYTECODE": "1",
                    "AZURE_CORE_COLLECT_TELEMETRY": "false",
                },
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
            samples.append(raw)
        return result, *samples


def login_observations(raw):
    result = {
        "stream": {
            "availability": "unavailable",
            "sampleBytes": None,
            "sampleUtf8Valid": None,
            "coverage": "unavailable",
            "debugPrefixSeen": False,
            "msalRecord": "absent",
            "unclassifiedLines": False,
        },
        **{
            key: set()
            for key in (
                "markers",
                "statuses",
                "aadstsCodes",
                "errorCodes",
                "exceptionClasses",
                "imdsStatuses",
            )
        },
        "unknownCodeSeen": False,
        "imdsPathSelected": False,
        "tokenAcquiredCheckpoint": False,
    }
    stream = result["stream"]
    if raw is None:
        return result
    oversized = len(raw) > PRIVATE_LIMIT
    stream["availability"] = (
        "over-limit" if oversized else "complete" if raw else "empty"
    )
    sample = raw[:PRIVATE_LIMIT]
    if oversized:
        # Never interpret a record cut by the sample boundary.
        sample = sample[: sample.rfind(b"\n") + 1]
    stream["sampleBytes"] = len(sample)
    try:
        text = sample.decode("utf-8")
        stream["sampleUtf8Valid"] = True
    except UnicodeDecodeError:
        text = sample.decode("utf-8", errors="replace")
        stream["sampleUtf8Valid"] = False
    recognized = False

    def literal(line):
        matched = False
        for label, pattern in MARKERS.items():
            if re.match(pattern, line):
                result["markers"].add(label)
                matched = True
        code = re.match(r"^(?:Code:\s*|\()([A-Za-z_]+)(?:\)|\s*$)", line)
        if code:
            record_code(code[1])
            matched = True
        aad = re.match(r"^AADSTS([0-9]+):", line)
        if aad:
            record_aadsts(aad[1])
            matched = True
        exception = re.match(
            r"^(?:(?:requests\.exceptions|urllib3\.exceptions|azure\.core\.exceptions|"
            r"azure\.cli\.core\.azclierror|msal\.managed_identity|knack\.util|"
            r"json\.decoder|jwt\.exceptions|builtins)\.)?([A-Za-z]+(?:Error|Exception|Timeout)):",
            line,
        )
        if exception and exception[1] in EXCEPTION_MARKERS:
            result["exceptionClasses"].add(exception[1])
            result["markers"].add(EXCEPTION_MARKERS[exception[1]])
            matched = True
        if line == "Traceback (most recent call last):":
            result["markers"].add("traceback-present")
            matched = True
        if line.startswith("The command failed with an unexpected error."):
            result["markers"].add("cli-unexpected-error")
            matched = True
        status = re.match(
            r"^(?:(?:Status Code|Status code|HTTP status|Response status):\s*"
            r"|Get Token request returned http error:\s*"
            r"|(?:requests\.exceptions\.)?HTTPError:\s*)([0-9]{3})(?:\s|,|$)",
            line,
        )
        if status and int(status[1]) in facts.HTTP_STATUSES:
            result["statuses"].add(int(status[1]))
            matched = True
        return matched

    def record_code(code):
        if isinstance(code, str) and code in ERROR_CODES:
            result["errorCodes"].add(code)
            result["markers"].add(ERROR_CODES[code])
        else:
            result["unknownCodeSeen"] = True

    def record_aadsts(code):
        if code in AADSTS:
            result["aadstsCodes"].add(code)
            result["markers"].add(AADSTS[code])
        else:
            result["unknownCodeSeen"] = True

    def record_body(body):
        if not isinstance(body, dict):
            return False
        if "error" in body:
            error = body["error"]
            record_code(error.get("code") if isinstance(error, dict) else error)
        if "error_codes" in body:
            codes = body["error_codes"]
            if isinstance(codes, list):
                for code in codes:
                    record_aadsts(str(code) if type(code) is int else "")
            else:
                result["unknownCodeSeen"] = True
        description = body.get("error_description")
        if isinstance(description, str):
            for line in description.splitlines():
                matched = literal(line.strip())
                stream["unclassifiedLines"] |= bool(line.strip()) and not matched
        elif "error_description" in body:
            stream["unclassifiedLines"] = True
        return any(key in body for key in ("error", "error_codes", "error_description"))

    for line in text.splitlines():
        line = re.sub(r"\x1b\[[0-9;]{0,20}m", "", line).strip()
        line = re.sub(r"^(?:ERROR|WARNING|DEBUG|INFO):\s*", "", line)
        logger = next((name for name in LOGGERS if line.startswith(name + ": ")), None)
        if logger:
            stream["debugPrefixSeen"] = True
            line = line[len(logger) + 2 :]
        matched = literal(line)
        if logger in LOGGERS[:2] and line.startswith("MSAL error: "):
            try:
                body = ast.literal_eval(line[len("MSAL error: ") :])
                parsed = record_body(body)
            except (ValueError, SyntaxError, RecursionError):
                parsed = False
            if not parsed:
                stream["msalRecord"] = "malformed"
            elif stream["msalRecord"] == "absent":
                stream["msalRecord"] = "parsed"
            matched = True
        if logger == "msal.managed_identity" and line == (
            "Obtaining token via managed identity on Azure VM"
        ):
            result["imdsPathSelected"] = True
            matched = True
        if logger in LOGGERS[2:4] and line == (
            "Managed identity: token was retrieved. Now trying to initialize local accounts..."
        ):
            result["tokenAcquiredCheckpoint"] = True
            matched = True
        if logger == "urllib3.connectionpool":
            status = re.match(
                r'^http://169\.254\.169\.254(?::80)? "GET '
                r'/metadata/identity/oauth2/token\?[^"\r\n ]* HTTP/1\.[01]" '
                r"([0-9]{3})(?: |$)",
                line,
            )
            if status and int(status[1]) in facts.HTTP_STATUSES:
                result["imdsStatuses"].add(int(status[1]))
                result["statuses"].add(int(status[1]))
                matched = True
        if line.startswith("{"):
            try:
                matched = record_body(json.loads(line)) or matched
            except (ValueError, RecursionError):
                matched = False
        recognized = recognized or matched
        stream["unclassifiedLines"] |= bool(line) and not matched
    if oversized:
        stream["coverage"] = "partial"
    elif not raw:
        stream["coverage"] = "empty"
    elif recognized:
        stream["coverage"] = "mixed" if stream["unclassifiedLines"] else "recognized"
    else:
        stream["coverage"] = "unrecognized"
    return result


def observe_login(report, stdout, stderr):
    observations = [login_observations(raw) for raw in (stdout, stderr)]
    report["loginOutput"] = dict(
        zip(("stdout", "stderr"), (row["stream"] for row in observations))
    )
    sets = {
        key: set.union(*(row[key] for row in observations))
        for key in (
            "markers",
            "statuses",
            "aadstsCodes",
            "errorCodes",
            "exceptionClasses",
            "imdsStatuses",
        )
    }
    report["loginFailureMarkers"] = sorted(sets["markers"]) or ["unclassified"]
    report["reportedHttpStatus"] = (
        next(iter(sets["statuses"])) if len(sets["statuses"]) == 1 else None
    )
    report["httpStatusConflict"] = len(sets["statuses"]) > 1
    report["loginEvidence"] = {
        **{
            key: sorted(sets[key])
            for key in ("aadstsCodes", "errorCodes", "exceptionClasses")
        },
        **{
            key: any(row[key] for row in observations)
            for key in (
                "unknownCodeSeen",
                "imdsPathSelected",
                "tokenAcquiredCheckpoint",
            )
        },
        "imdsHttpStatus": (
            next(iter(sets["imdsStatuses"])) if len(sets["imdsStatuses"]) == 1 else None
        ),
        "imdsHttpStatusConflict": len(sets["imdsStatuses"]) > 1,
    }


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
            if row["state"] in ("timeout", "cancelled"):
                code = row["exitCode"]
                report["status"] = row["state"]
            if (
                name == "version"
                and row["state"] == "ok"
                and stdout is not None
                and len(stdout) <= PRIVATE_LIMIT
            ):
                try:
                    version = json.loads(stdout).get("azure-cli")
                    if isinstance(version, str) and re.fullmatch(
                        r"[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}", version
                    ):
                        report["cliVersion"] = version
                except (ValueError, AttributeError, RecursionError):
                    report["cliVersion"] = None
            if (
                name == "help"
                and row["state"] == "ok"
                and stdout is not None
                and len(stdout) <= PRIVATE_LIMIT
            ):
                report["argumentSupport"] = {
                    flag: bool(
                        re.search(
                            rb"(?m)^[ \t]*--" + flag.encode() + rb"(?:[ \t,:=]|$)",
                            stdout,
                        )
                    )
                    for flag in ARGUMENTS
                }
            if name == "login":
                observe_login(report, stdout, stderr)
                if row["state"] == "ok":
                    report["loginFailureMarkers"] = []
            if row["state"] in ("timeout", "cancelled"):
                break
            if row["exitCode"] is None:
                break
            if name == "login":
                if row["state"] == "ok":
                    report["status"] = "login-succeeded-not-guest-execution"
                    code = 0
            publish(output, report)
    except KeyboardInterrupt:
        if report["status"] not in ("timeout", "cancelled"):
            report["status"] = "cancelled"
            code = 130
        report["exceptionClass"] = "KeyboardInterrupt"
    except Exception as error:
        if report["status"] not in ("timeout", "cancelled"):
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
