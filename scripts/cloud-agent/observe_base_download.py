#!/usr/bin/env python3

"""Child-local observation of the pinned Azure CLI Universal Package path."""

import hashlib
import importlib.util
import json
import os
import re
import runpy
import shutil
import subprocess
import sys
import types
import zipfile

from pathlib import Path
from contextlib import contextmanager

LIMIT = 4096
CLI_SHA256 = "6202f37a66f6092d07ea7a9f05368dc3ffe7d93263482e1023d759a62560fda7"
# Extension 1.0.2 release 20250624.2. The official build strips SDK docstrings
# and header comments; pin released wheel bytes, not normalized Git source.
CONTRACTS = {
    "dev.artifacts.universal": (
        "b479d4e943c49321d2fc046aefc98cc9195d263052632eebe30f0043cc7a369f",
        {"download_package": "cli-download"},
    ),
    "dev.common.artifacttool_updater": (
        "14a9c5b1e4437592caa30a50f405b7199e46dd31c6c6adf712ada66e3df4ee5f",
        {
            "_get_current_release": "release-lookup",
            "ArtifactToolUpdater._get_artifacttool": "tool-bootstrap",
            "_update_artifacttool": "tool-download-extract",
        },
    ),
    "dev.common.artifacttool": (
        "5663b624365b6b4872dda031c6fb489aab6cede0934109bd747cbc827eec9190",
        {"ArtifactToolInvoker.run_artifacttool": "tool-invocation"},
    ),
    "dev.common.services": (
        "6f4aa0a2e746e3c70bff73c804217088e7f042f01a77cce3b029d9d8ac4731b1",
        {"_get_credentials": "credential-resolution"},
    ),
    "dev.common.external_tool": (
        "c686a5243e9dba617a11c9c81c0360948f00a51a824270c73b210dc6a82ebe11",
        {
            "ExternalToolInvoker.start": "tool-spawn",
            "ExternalToolInvoker.wait": "tool-wait",
        },
    ),
    "devops_sdk.client": (
        "99aa577e32e9a9f0084ef7884e40b92e7897a58a87e41a9a15cdf9c1c431956b",
        {"Client._handle_error": "sdk-http-error"},
    ),
}
PHASES = {phase for _, functions in CONTRACTS.values() for phase in functions.values()}
SOURCE_GATES = {
    "dev.artifacts.universal": "extension-universal-source",
    "dev.common.artifacttool_updater": "extension-updater-source",
    "dev.common.artifacttool": "extension-artifacttool-source",
    "dev.common.services": "extension-services-source",
    "dev.common.external_tool": "extension-external-tool-source",
    "devops_sdk.client": "extension-sdk-client-source",
}
GATES = {
    "runtime-implementation",
    "runtime-python-version",
    "runtime-trace",
    "cli-main-discovery",
    "cli-main-source",
    "cli-main-code",
    "extension-root",
    "extension-contract",
    *SOURCE_GATES.values(),
    *(phase + "-function" for phase in PHASES),
}
GATE_REASONS = {
    "unavailable",
    "unreadable",
    "too-large",
    "hash-mismatch",
    "compile-unavailable",
    "function-unavailable",
    "code-mismatch",
    "binding-mismatch",
}
KINDS = {
    TypeError: "TypeError",
    KeyError: "KeyError",
    ValueError: "ValueError",
    OSError: "OSError",
    FileNotFoundError: "FileNotFoundError",
    PermissionError: "PermissionError",
    zipfile.BadZipFile: "BadZipFile",
}
BOOLEANS = (
    "releasePresent",
    "contentLengthValid",
    "zipOpened",
    "executablePresent",
    "spawnSucceeded",
)


def unavailable(status="unavailable"):
    return {
        "status": status,
        "gate": None,
        "phases": dict.fromkeys(sorted(PHASES)),
        **dict.fromkeys(BOOLEANS),
        # No fixed, source-evidenced service-code catalog is established.
        "serviceCode": None,
        "innerExitCode": None,
    }


def rejected(gate, reason, observed_sha256=None):
    result = unavailable("source-contract-unavailable")
    result["gate"] = {
        "id": gate,
        "reason": reason,
        "observedSha256": observed_sha256,
    }
    return result


class GateFailure(ValueError):
    def __init__(self, gate, reason, observed_sha256=None):
        super().__init__("source-contract-unavailable")
        self.observation = rejected(gate, reason, observed_sha256)


def validate(value):
    expected = set(unavailable())
    if type(value) is not dict or set(value) != expected:
        return unavailable("invalid-channel")
    if value["status"] not in (
        "available",
        "unavailable",
        "source-contract-unavailable",
        "observer-error",
    ):
        return unavailable("invalid-channel")
    gate = value["gate"]
    if gate is not None:
        if (
            value["status"] != "source-contract-unavailable"
            or type(gate) is not dict
            or set(gate) != {"id", "reason", "observedSha256"}
            or type(gate["id"]) is not str
            or gate["id"] not in GATES
            or type(gate["reason"]) is not str
            or gate["reason"] not in GATE_REASONS
        ):
            return unavailable("invalid-channel")
        source_gate = gate["id"] in {"cli-main-source", *SOURCE_GATES.values()}
        function_gate = gate["id"] in {phase + "-function" for phase in PHASES}
        allowed = {"unavailable"}
        if source_gate:
            allowed = {
                "unavailable",
                "unreadable",
                "too-large",
                "hash-mismatch",
                "compile-unavailable",
            }
        elif function_gate:
            allowed = {"function-unavailable", "code-mismatch", "binding-mismatch"}
        elif gate["id"] == "cli-main-code":
            allowed = {"code-mismatch"}
        if gate["reason"] not in allowed:
            return unavailable("invalid-channel")
        digest = gate["observedSha256"]
        if gate["reason"] == "hash-mismatch" and digest is None:
            return unavailable("invalid-channel")
        if digest is not None and (
            type(digest) is not str
            or re.fullmatch(r"[0-9a-f]{64}", digest) is None
            or gate["id"] not in {"cli-main-source", *SOURCE_GATES.values()}
            or gate["reason"] != "hash-mismatch"
        ):
            return unavailable("invalid-channel")
    phases = value["phases"]
    if type(phases) is not dict or set(phases) != PHASES:
        return unavailable("invalid-channel")
    for row in phases.values():
        if row is None:
            continue
        if (
            type(row) is not dict
            or set(row) != {"entered", "returned", "exceptionKind", "httpStatus"}
            or row["entered"] is not True
            or (row["returned"] is not None and type(row["returned"]) is not bool)
            or row["exceptionKind"] not in (None, "unavailable", *KINDS.values())
        ):
            return unavailable("invalid-channel")
        status = row["httpStatus"]
        if status is not None and (type(status) is not int or not 100 <= status <= 599):
            return unavailable("invalid-channel")
    for key in BOOLEANS:
        if value[key] is not None and type(value[key]) is not bool:
            return unavailable("invalid-channel")
    for key, low, high in (("innerExitCode", -64, 255),):
        item = value[key]
        if item is not None and (type(item) is not int or not low <= item <= high):
            return unavailable("invalid-channel")
    if value["serviceCode"] is not None:
        return unavailable("invalid-channel")
    empty = unavailable(value["status"])
    empty["gate"] = gate
    if value["status"] != "available" and value != empty:
        return unavailable("invalid-channel")
    return value


def read_channel(fd):
    if fd is None:
        return unavailable()
    try:
        body = os.read(fd, LIMIT + 1)
        if not body:
            return unavailable()
        if len(body) > LIMIT:
            return unavailable("invalid-channel")
        return validate(json.loads(body))
    except BlockingIOError:
        return unavailable()
    except (OSError, ValueError, UnicodeError):
        return unavailable("invalid-channel")


def launch(argv, fd):
    """Recognize only the official 2.91.0 Debian isolated launcher."""
    launcher = Path(argv[0])
    if not launcher.is_absolute() or launcher != launcher.resolve():
        return None
    expected = (
        b'#!/usr/bin/env bash\nbin_dir=`cd "$(dirname "$BASH_SOURCE[0]")"; pwd`\n'
        b'AZ_INSTALLER=DEB "$bin_dir"/../../opt/az/bin/python3 -Im azure.cli "$@"\n'
    )
    with launcher.open("rb") as source:
        if source.read(len(expected) + 1) != expected:
            return None
    interpreter = launcher.parent / "../../opt/az/bin/python3"
    if not interpreter.is_file():
        return None
    return [
        str(interpreter),
        "-I",
        str(Path(__file__).resolve()),
        str(fd),
        *argv,
    ]


@contextmanager
def invocation(argv):
    if argv[:4] != ["az", "artifacts", "universal", "download"]:
        yield argv, (), None
        return
    reader, writer = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        try:
            executable = shutil.which("az")
            wrapped = launch([executable, *argv[1:]], writer) if executable else None
        except (OSError, ValueError):
            wrapped = None
        yield (wrapped, (writer,), reader) if wrapped else (argv, (), None)
    finally:
        os.close(writer)
        os.close(reader)


def checked_source(path, digest, gate):
    try:
        with path.open("rb") as source:
            body = source.read(64 * 1024 + 1)
    except OSError:
        raise GateFailure(gate, "unreadable")
    if len(body) > 64 * 1024:
        raise GateFailure(gate, "too-large")
    observed = hashlib.sha256(body).hexdigest()
    if observed != digest:
        raise GateFailure(gate, "hash-mismatch", observed)
    try:
        return compile(body, str(path), "exec")
    except Exception:
        raise GateFailure(gate, "compile-unavailable")


def function_codes(code, prefix=""):
    result = {}
    for item in code.co_consts:
        if isinstance(item, types.CodeType):
            name = prefix + item.co_name
            result[name] = item
            result.update(function_codes(item, name + "."))
    return result


def instance_fields(value):
    # Never invoke an SDK property, format an object or retain credential locals.
    cls = type(value)
    if type(cls) is not type:
        return {}
    for base in type.__getattribute__(cls, "__mro__"):
        descriptor = type.__getattribute__(base, "__dict__").get("__dict__")
        if descriptor is not None:
            if type(descriptor) is not types.GetSetDescriptorType:
                return {}
            fields = descriptor.__get__(value, cls)
            return fields if type(fields) is dict else {}
    return {}


class Observer:
    def __init__(self, root):
        self.result = unavailable("available")
        self.owner = os.getpid()
        self.targets = {}
        self.bindings = {}
        self.pending = {}
        self.entrypoint_code = None
        for suffix, (digest, functions) in CONTRACTS.items():
            module = "azext_devops." + suffix
            gate = SOURCE_GATES[suffix]
            try:
                path = (root / (module.replace(".", "/") + ".py")).resolve()
                codes = function_codes(checked_source(path, digest, gate))
            except GateFailure:
                raise
            except Exception:
                raise GateFailure(gate, "unavailable")
            for name, phase in functions.items():
                if name not in codes:
                    raise GateFailure(phase + "-function", "function-unavailable")
            self.bindings[module] = {
                name: (codes[name], phase) for name, phase in functions.items()
            }
            for name, phase in functions.items():
                self.targets[(module, codes[name].co_name)] = (codes[name], phase)

    def trace(self, frame, event, arg):
        if os.getpid() != self.owner or self.result["status"] != "available":
            return None
        if (
            self.entrypoint_code is not None
            and frame.f_globals.get("__name__") == "__main__"
            and frame.f_code.co_name == "<module>"
        ):
            if frame.f_code != self.entrypoint_code:
                self.result = rejected("cli-main-code", "code-mismatch")
            return None
        target = self.targets.get(
            (frame.f_globals.get("__name__"), frame.f_code.co_name)
        )
        if target is None:
            return None
        code, phase = target
        if frame.f_code != code:
            self.result = rejected(phase + "-function", "code-mismatch")
            return None
        mismatch = self.runtime_mismatch(frame.f_globals)
        if mismatch is not None:
            self.result = rejected(mismatch, "binding-mismatch")
            return None
        try:
            self.observe(frame, event, arg, phase)
        except Exception:
            self.result = unavailable("observer-error")
            return None
        return self.trace

    def runtime_mismatch(self, namespace):
        for name, (code, phase) in self.bindings[namespace["__name__"]].items():
            parts = name.split(".")
            value = namespace.get(parts[0])
            if len(parts) == 2:
                if type(value) is not type:
                    return phase + "-function"
                value = type.__getattribute__(value, "__dict__").get(parts[1])
            if type(value) is not types.FunctionType or value.__code__ != code:
                return phase + "-function"
        return None

    def observe(self, frame, event, arg, phase):
        row = self.result["phases"][phase]
        if row is None:
            row = {
                "entered": True,
                "returned": None,
                "exceptionKind": None,
                "httpStatus": None,
            }
            self.result["phases"][phase] = row
        if event == "exception":
            row["exceptionKind"] = KINDS.get(arg[0], "unavailable")
            self.pending[id(frame)] = True
        elif event == "call":
            self.pending[id(frame)] = False
        if event == "return":
            # Cleanup line events do not prove recovery. After an exception,
            # trace-return(None) cannot distinguish recovery from unwinding.
            had_exception = self.pending.pop(id(frame), False)
            row["returned"] = None if had_exception and arg is None else True
            if phase == "release-lookup" and row["returned"]:
                self.result["releasePresent"] = type(arg) is tuple and len(arg) == 2
        if phase not in (
            "tool-download-extract",
            "sdk-http-error",
            "tool-spawn",
            "tool-wait",
        ):
            return
        local = frame.f_locals
        if phase == "tool-download-extract":
            if "content_length" in local:
                length = local["content_length"]
                self.result["contentLengthValid"] = type(length) is int and length > 0
            elif (
                event == "exception"
                and arg[0] in (KeyError, ValueError)
                and "response" in local
            ):
                self.result["contentLengthValid"] = False
            if type(local.get("f")) is zipfile.ZipFile:
                self.result["zipOpened"] = True
            elif event == "exception" and arg[0] is zipfile.BadZipFile:
                self.result["zipOpened"] = False
            # These locals exist only after the pinned os.path.exists branch.
            if "artifacttool_stat" in local:
                self.result["executablePresent"] = True
            elif "artifacttool_binary" in local and "release_dir" in local:
                self.result["executablePresent"] = False
            response = instance_fields(local.get("response"))
            status = response.get("status_code")
            if type(status) is int and 100 <= status <= 599:
                row["httpStatus"] = status
        if phase == "sdk-http-error":
            status = instance_fields(local.get("response")).get("status_code")
            if type(status) is int and 100 <= status <= 599:
                row["httpStatus"] = status
        if phase in ("tool-spawn", "tool-wait"):
            # Exact pinned ExternalToolInvoker.start Popen assignment.
            if phase == "tool-spawn" and event == "line" and frame.f_lineno == 33:
                self.result["spawnSucceeded"] = False
            proc = instance_fields(local.get("self")).get("_proc")
            if type(proc) is subprocess.Popen:
                if phase == "tool-spawn":
                    self.result["spawnSucceeded"] = True
                status = instance_fields(proc).get("returncode")
                if type(status) is int and -64 <= status <= 255:
                    self.result["innerExitCode"] = status


def emit(fd, value):
    body = json.dumps(validate(value), separators=(",", ":")).encode("ascii")
    if len(body) > LIMIT:
        body = json.dumps(unavailable("observer-error")).encode("ascii")
    try:
        os.write(fd, body)
    except OSError:
        # Diagnostic publication must never replace CLI exit/cancellation.
        return


def main():
    fd = int(sys.argv[1])
    argv = sys.argv[2:]
    os.set_inheritable(fd, False)
    os.set_blocking(fd, False)
    owner = os.getpid()
    gate = "runtime-implementation"
    try:
        if sys.implementation.name != "cpython":
            raise GateFailure(gate, "unavailable")
        gate = "runtime-python-version"
        if not (3, 10) <= sys.version_info[:2] <= (3, 14):
            raise GateFailure(gate, "unavailable")
        gate = "runtime-trace"
        if sys.gettrace() is not None:
            raise GateFailure(gate, "unavailable")
        gate = "cli-main-discovery"
        spec = importlib.util.find_spec("azure.cli.__main__")
        if spec is None or spec.origin is None:
            raise GateFailure(gate, "unavailable")
        gate = "cli-main-source"
        entrypoint_code = checked_source(Path(spec.origin).resolve(), CLI_SHA256, gate)
        gate = "extension-root"
        root = Path(os.environ["AZURE_EXTENSION_DIR"]) / "azure-devops"
        gate = "extension-contract"
        observer = Observer(root)
        observer.entrypoint_code = entrypoint_code
    except Exception as error:
        observation = (
            error.observation
            if type(error) is GateFailure
            else rejected(gate, "unavailable")
        )
        emit(fd, observation)
        os.close(fd)
        os.execv(argv[0], argv)
    # Mirror the official Debian launcher. No authentication/config overrides.
    os.environ["AZ_INSTALLER"] = "DEB"
    sys.argv = argv
    sys.settrace(observer.trace)
    try:
        runpy.run_module("azure.cli", run_name="__main__", alter_sys=True)
    finally:
        sys.settrace(None)
        if os.getpid() == owner:
            emit(fd, observer.result)
            os.close(fd)


if __name__ == "__main__":
    main()
