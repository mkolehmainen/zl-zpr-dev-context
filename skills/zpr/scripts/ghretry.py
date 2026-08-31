"""Minimal bounded-retry wrapper for `gh` calls (shared by the sibling scripts).

3 attempts, 2s/4s backoff. Only transient faults retry (DNS, connection,
timeout, 5xx, connection reset); auth errors, 404s and rate-limit exhaustion
are permanent and fail immediately — retrying them just burns time and buries
the real message.

All diagnostics go to stderr. Stdout is reserved for the caller's payload,
so this module is safe to use from output-hash-based monitor scripts.
"""

from __future__ import annotations

import subprocess
import sys
import time

ATTEMPTS = 3
BACKOFF = (2, 4)  # seconds before attempt 2 and 3

TRANSIENT_MARKERS = (
    "temporary failure in name resolution",
    "could not resolve host",
    "error connecting to",
    "connection refused",
    "connection reset",
    "timed out",
    "timeout",
    "tls handshake",
    "502 ",
    "503 ",
    "504 ",
    "http 502",
    "http 503",
    "http 504",
)
PERMANENT_MARKERS = (
    "bad credentials",
    "http 404",
    "404 not found",
    "api rate limit exceeded",
    "missing required scopes",
)


def _classify(stderr: str) -> str:
    low = stderr.lower()
    if any(m in low for m in PERMANENT_MARKERS):
        return "permanent"
    if any(m in low for m in TRANSIENT_MARKERS):
        return "transient"
    return "permanent"


def run_gh(args: list[str]) -> str:
    """Run `gh <args>` and return stdout; raise RuntimeError on failure."""
    last_err = ""
    for attempt in range(1, ATTEMPTS + 1):
        proc = subprocess.run(
            ["gh", *args], capture_output=True, text=True
        )
        if proc.returncode == 0:
            return proc.stdout
        last_err = proc.stderr.strip()
        if _classify(last_err) == "permanent" or attempt == ATTEMPTS:
            break
        delay = BACKOFF[min(attempt - 1, len(BACKOFF) - 1)]
        print(
            f"gh attempt {attempt}/{ATTEMPTS} failed (transient), "
            f"retrying in {delay}s: {last_err}",
            file=sys.stderr,
        )
        time.sleep(delay)
    raise RuntimeError(f"gh {' '.join(args[:2])} failed: {last_err}")


def gh_login() -> str:
    """Login of the currently authenticated gh user."""
    return run_gh(["api", "user", "-q", ".login"]).strip()
