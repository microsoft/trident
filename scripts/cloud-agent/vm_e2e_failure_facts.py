"""Closed, task-specific failure facts. Never serialize exception messages."""

import json
import os
import stat

from contextlib import contextmanager
from http import HTTPStatus
from urllib.error import HTTPError

OPERATIONS = frozenset(
    """
    publication-not-started privileged-context cancelled-context publication-finalize
    checkpoint-publication report-publication host-admission ambient-credentials
    harness-argument harness-read harness-bind storage-mount private-runtime-create
    packet-read packet-pin packet-schema packet-producer packet-review packet-copy
    identity-extension identity-umi-login prepare-process manifest-digest
    bundle-verify lineage-bind host-setup host-lock host-check inventory-domains
    inventory-networks inventory-pools empty-guest-guard
    qemu-search-var qemu-search-var-lib qemu-search-libvirt
    scenario-process scenario-results bundle-recheck execution-diagnostics
    preparation-diagnostics final-host-lock final-inventory-domains
    final-inventory-networks final-inventory-pools
    inventory-compare signal-restore
    acquire-arguments acquire-token acquire-build acquire-timeline acquire-stages
    acquire-log-metadata qualify-packet qualify-logs qualify-rpms qualify-artifacts
    acquire-artifact-list acquire-output-create prepare-tools prepare-installers
    prepare-images prepare-image-verify prepare-certificate manifest-write
    manifest-verify manifest-promote
    acquire-staging-cleanup signal-install
    host-platform host-os-release host-kvm host-tool-sudo host-tool-virsh
    host-tool-qemu-system-x86_64 host-libvirt
    """.split()
    + [
        "prerequisite-" + name
        for name in (
            "az",
            "git",
            "sudo",
            "bash",
            "mountpoint",
            "apt-get",
            "pip3",
            "runuser",
        )
    ]
    + [
        prefix + resource
        for prefix in ("inventory-validate-", "final-inventory-validate-")
        for resource in ("domains", "networks", "pools")
    ]
    + [
        action + "-" + name
        for action in ("artifact-validate", "artifact-download", "artifact-extract")
        for name in (
            "go-tools",
            "trident-installer",
            "trident-split-installer",
            "trident-testimage",
            "trident-verity-testimage",
            "trident-usrverity-testimage",
        )
    ]
)
EXCEPTIONS = frozenset(
    """
    ArtifactError OSError FileNotFoundError PermissionError FileExistsError
    IsADirectoryError NotADirectoryError TimeoutError ValueError JSONDecodeError
    UnicodeDecodeError UnicodeEncodeError KeyError TypeError SubprocessError
    CalledProcessError TimeoutExpired BadZipFile Failure ProcessFailure
    KeyboardInterrupt Cancelled HTTPError URLError ConnectionError
    ConnectionRefusedError ConnectionResetError ConnectionAbortedError
    BrokenPipeError InterruptedError BlockingIOError ProcessLookupError
    ParseError
    """.split()
)
HTTP_STATUSES = frozenset(status.value for status in HTTPStatus)
KEYS = {"operation", "exitCode", "exceptionClass", "httpStatus"}
LIMIT = 2048


def validate(value):
    if (
        not isinstance(value, dict)
        or set(value) != KEYS
        or not isinstance(value["operation"], str)
        or value["operation"] not in OPERATIONS
        or not (
            value["exitCode"] is None
            or type(value["exitCode"]) is int
            and -128 <= value["exitCode"] <= 255
        )
        or value["exceptionClass"] is not None
        and (
            not isinstance(value["exceptionClass"], str)
            or value["exceptionClass"] not in EXCEPTIONS
        )
        or not (
            value["httpStatus"] is None
            or type(value["httpStatus"]) is int
            and value["httpStatus"] in HTTP_STATUSES
        )
    ):
        raise ValueError("invalid bounded failure fact")
    return dict(value)


def fact(operation, error=None, exit_code=None):
    value = dict.fromkeys(KEYS)
    value["operation"] = operation
    seen = set()
    for _ in range(8):
        if error is None or id(error) in seen:
            break
        seen.add(id(error))
        name = type(error).__name__
        if name in EXCEPTIONS:
            value["exceptionClass"] = name
        if isinstance(error, HTTPError) and type(error.code) is int:
            if error.code in HTTP_STATUSES:
                value["httpStatus"] = error.code
        code = getattr(error, "returncode", None)
        if type(code) is int and -128 <= code <= 255:
            value["exitCode"] = code
        error = error.__cause__
    if type(exit_code) is int and -128 <= exit_code <= 255:
        value["exitCode"] = exit_code
    return validate(value)


class Trace:
    def __init__(self, emit):
        self.emit = emit
        self.operation = "publication-not-started"
        self.failure = None
        self.error = None
        self.cleanup_failure = None

    def start(self, operation):
        value = fact(operation)
        self.operation = operation
        try:
            self.emit("started", value)
        except BaseException:
            self.operation = "checkpoint-publication"
            raise

    def fail(self, error=None, exit_code=None, *, secondary=False):
        if secondary:
            if self.cleanup_failure is None:
                self.cleanup_failure = fact(self.operation, error, exit_code)
            self.emit("cleanup-failed", self.cleanup_failure)
            return
        if self.failure is None:
            self.failure = fact(self.operation, error, exit_code)
            self.error = error
        self.emit("failed", self.failure)

    def complete(self):
        self.emit("completed", fact(self.operation))

    def adopt(self, value):
        if self.failure is None:
            self.failure = validate(value)
        self.emit("failed", self.failure)


def mark(observe, operation):
    if observe is not None:
        observe(operation)


@contextmanager
def retaining(on_failure, observe, cleanup_operation):
    try:
        yield
    except BaseException as error:
        if on_failure is not None:
            on_failure(error)
        raise
    finally:
        mark(observe, cleanup_operation)


@contextmanager
def private_handoff(path):
    """Create once in the private runtime directory, then write only the pinned FD."""
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    fd = None
    try:
        info = os.fstat(directory)
        if info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise ValueError("private handoff directory required")
        fd = os.open(
            path.name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
            0o600,
            dir_fd=directory,
        )

        retained = {"state": "started", "fact": None, "cleanupFailure": None}

        def emit(state, value):
            if state not in ("started", "failed", "completed", "cleanup-failed"):
                raise ValueError("invalid handoff state")
            value = validate(value)
            if state == "cleanup-failed":
                retained["cleanupFailure"] = value
            elif retained["state"] != "failed":
                retained.update(state=state, fact=value)
            raw = json.dumps(retained).encode()
            if len(raw) > LIMIT:
                raise ValueError("handoff limit")
            with os.fdopen(os.dup(fd), "wb") as stream:
                stream.seek(0)
                stream.write(raw)
                stream.flush()
                stream.truncate()

        yield emit
    finally:
        if fd is not None:
            os.close(fd)
        os.close(directory)


def read_handoff(raw):
    if len(raw) > LIMIT:
        raise ValueError("handoff limit")
    value = json.loads(raw)
    if (
        not isinstance(value, dict)
        or set(value) != {"state", "fact", "cleanupFailure"}
        or value["state"] not in ("started", "failed", "completed")
    ):
        raise ValueError("invalid handoff")
    value["fact"] = validate(value["fact"])
    if value["cleanupFailure"] is not None:
        value["cleanupFailure"] = validate(value["cleanupFailure"])
        if value["cleanupFailure"]["operation"] != "acquire-staging-cleanup":
            raise ValueError("not a preparer cleanup operation")
    operation = value["fact"]["operation"]
    if not (
        operation.startswith(("acquire-", "qualify-", "artifact-"))
        or operation
        in {
            "prepare-tools",
            "prepare-installers",
            "prepare-images",
            "prepare-image-verify",
            "prepare-certificate",
            "manifest-write",
            "manifest-verify",
            "manifest-promote",
        }
    ):
        raise ValueError("not a preparer operation")
    if value["state"] != "failed" and value["fact"] != fact(value["fact"]["operation"]):
        raise ValueError("started is not a failure")
    return value


def load_handoff(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_size > LIMIT
            or info.st_uid != os.geteuid()
            or stat.S_IMODE(info.st_mode) != 0o600
            or info.st_nlink != 1
        ):
            raise ValueError("bounded private handoff required")
        return read_handoff(stream.read(LIMIT + 1))
