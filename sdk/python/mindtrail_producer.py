"""Bounded, shell-free bridge to the MindTrail producer executable."""
from __future__ import annotations

import json
import math
import os
import subprocess
import threading
from typing import Any, Literal, TypedDict, Union, cast


class EventV1(TypedDict):
    v: Literal[1]
    session: str
    seq: int
    ts: str
    type: str
    payload: dict[str, Any]


class Receipt(TypedDict):
    accepted: int
    duplicates: int
    last_seq: int


class Queued(TypedDict):
    status: Literal["queued"]
    batch_id: str
    session: str


class Committed(TypedDict):
    status: Literal["committed"]
    batch_id: str
    session: str
    receipt: Receipt


class _RequiredError(TypedDict):
    code: str
    message: str


class ErrorDetail(_RequiredError, total=False):
    index: int
    last_seq: int
    expected_seq: int
    offending_seq: int


class ProducerError(TypedDict):
    error: ErrorDetail


class FlushCounts(TypedDict):
    committed: int
    pending: int
    blocked: int


SubmitResult = Union[Queued, Committed, ProducerError]
FlushResult = Union[FlushCounts, ProducerError]


def _error(code: str, message: str) -> dict[str, Any]:
    return {"error": {"code": code, "message": message}}


def _failed() -> dict[str, Any]:
    return _error("producer_failed", "The producer could not complete the request.")


def _json_safe(value: Any, seen: set[int]) -> bool:
    if value is None or type(value) in (str, bool):
        return True
    if type(value) is int:
        return abs(value) <= 2**53 - 1
    if type(value) is float:
        return math.isfinite(value) and (not value.is_integer() or abs(value) <= 2**53 - 1)
    if type(value) not in (list, dict) or id(value) in seen:
        return False
    seen.add(id(value))
    try:
        if type(value) is list:
            return all(_json_safe(item, seen) for item in value)
        return all(type(key) is str and _json_safe(item, seen) for key, item in value.items())
    finally:
        seen.remove(id(value))


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def _safe_int(value: Any) -> bool:
    return type(value) is int and 0 <= value <= 2**53 - 1


def _expected_batch(events: list[EventV1]) -> tuple[str, int, int] | None:
    if not 1 <= len(events) <= 200:
        return None
    session = None
    last_seq = 0
    for event in events:
        if type(event) is not dict or type(event.get("session")) is not str or not event["session"].strip() or (session is not None and event["session"] != session):
            return None
        seq = event.get("seq")
        if not _safe_int(seq) or seq <= last_seq:
            return None
        session, last_seq = event["session"], seq
    return (session, len(events), last_seq)


def _valid(value: Any, command: str, code: int, batch_id: str | None, expected: tuple[str, int, int] | None) -> bool:
    if type(value) is not dict:
        return False
    if "error" in value:
        error = value["error"]
        return (code == 1 and set(value) == {"error"} and type(error) is dict
                and type(error.get("code")) is str and bool(error["code"])
                and type(error.get("message")) is str
                and all(key in ("code", "message") or (key in ("index", "last_seq", "expected_seq", "offending_seq") and _safe_int(item)) for key, item in error.items()))
    if code != 0:
        return False
    if command == "flush":
        return set(value) == {"committed", "pending", "blocked"} and all(_safe_int(value[k]) for k in value)
    if expected is None or type(value.get("batch_id")) is not str or value["batch_id"] != batch_id or value.get("session") != expected[0]:
        return False
    if value.get("status") == "queued":
        return set(value) == {"status", "batch_id", "session"}
    receipt = value.get("receipt")
    return (value.get("status") == "committed" and set(value) == {"status", "batch_id", "session", "receipt"}
            and type(receipt) is dict and set(receipt) == {"accepted", "duplicates", "last_seq"}
            and all(_safe_int(v) for v in receipt.values())
            and receipt["accepted"] + receipt["duplicates"] == expected[1] and receipt["last_seq"] >= expected[2])


class MindTrailProducer:
    def __init__(self, executable_path: str, *, timeout: float = 10.0, max_output_bytes: int = 65_536):
        self.executable_path = os.fspath(executable_path)
        self.timeout = timeout
        self.max_output_bytes = max_output_bytes

    def submit_events(self, queue: str, batch_id: str, events: list[EventV1]) -> SubmitResult:
        try:
            if type(events) is not list or not _json_safe(events, set()):
                raise ValueError("invalid JSON")
            payload = json.dumps(events, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8")
        except (TypeError, ValueError, UnicodeError, RecursionError, OverflowError):
            return cast(ProducerError, _error("invalid_request", "The events could not be encoded as JSON."))
        return cast(SubmitResult, self._run("submit-events", ["--queue", queue, "--batch-id", batch_id], payload, _expected_batch(events)))

    def flush(self, queue: str) -> FlushResult:
        return cast(FlushResult, self._run("flush", ["--queue", queue], b""))

    def _run(self, command: str, args: list[str], payload: bytes, expected: tuple[str, int, int] | None = None) -> dict[str, Any]:
        if not os.path.isabs(self.executable_path) or not isinstance(self.timeout, (int, float)) or not math.isfinite(self.timeout) or self.timeout <= 0 or type(self.max_output_bytes) is not int or self.max_output_bytes <= 0:
            return _failed()
        try:
            proc = subprocess.Popen([self.executable_path, command, *args], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, shell=False, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        except (OSError, ValueError, TypeError):
            return _failed()
        exceeded = threading.Event()
        stdout = bytearray()

        def read(stream: Any, target: bytearray | None) -> None:
            total = 0
            try:
                while True:
                    chunk = os.read(stream.fileno(), 4096)
                    if not chunk:
                        break
                    total += len(chunk)
                    if total > self.max_output_bytes:
                        exceeded.set()
                        try:
                            proc.kill()
                        except OSError:
                            pass
                        break
                    if target is not None:
                        target.extend(chunk)
            except (OSError, ValueError):
                pass

        def write() -> None:
            try:
                assert proc.stdin is not None
                proc.stdin.write(payload)
                proc.stdin.close()
            except (OSError, ValueError):
                pass

        threads = [threading.Thread(target=read, args=(proc.stdout, stdout), daemon=True),
                   threading.Thread(target=read, args=(proc.stderr, None), daemon=True),
                   threading.Thread(target=write, daemon=True)]
        for thread in threads:
            thread.start()
        try:
            code = proc.wait(timeout=self.timeout)
        except subprocess.TimeoutExpired:
            try:
                proc.kill()
            except OSError:
                pass
            try:
                proc.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
            return _failed()
        finally:
            for thread in threads:
                thread.join(timeout=1)
            for stream in (proc.stdin, proc.stdout, proc.stderr):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError:
                        pass
        if exceeded.is_set():
            return _failed()
        try:
            value = json.loads(stdout.decode("utf-8"), object_pairs_hook=_unique_object,
                               parse_constant=lambda _: (_ for _ in ()).throw(ValueError("invalid JSON constant")))
            return value if _valid(value, command, code, args[3] if command == "submit-events" else None, expected) else _failed()
        except (ValueError, UnicodeError, TypeError):
            return _failed()
