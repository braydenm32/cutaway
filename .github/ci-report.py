#!/usr/bin/env python3
"""Turns the errors in a failed CI step's output into GitHub annotations, so
they're listed on the run's summary page (and readable through GitHub's API)
without opening the full log.

Usage: python3 .github/ci-report.py ci-logs
Reports on the newest log in the folder: the step that failed (the steps
after it are skipped, so they write nothing).
"""

import pathlib
import re
import sys

MAX_CHARS = 6000
MAX_NOTES = 9


def escape(text: str) -> str:
    return text.replace("%", "%25").replace("\r", "").replace("\n", "%0A")


def note(title: str, body: str) -> None:
    body = body.strip()
    if len(body) > MAX_CHARS:
        body = body[:MAX_CHARS] + "\n…"
    title = escape(title).replace(",", "%2C").replace(":", "%3A")
    print(f"::error title={title}::{escape(body)}")


def problems(lines: list[str]) -> list[tuple[str, str]]:
    found = []
    i = 0
    while i < len(lines):
        line = lines[i]
        # A compiler error, up to the blank line after it.
        if re.match(r"^error(\[E\d+\])?: ", line) and not any(s in line for s in ("aborting due to", "could not compile", "test failed")):
            j = i + 1
            while j < len(lines) and lines[j].strip():
                j += 1
            found.append(("Build error", "\n".join(lines[i:j])))
            i = j
            continue
        # A failed test's captured output.
        m = re.match(r"^---- (.+) stdout ----$", line)
        if m:
            j = i + 1
            while j < len(lines) and not lines[j].startswith("---- ") and lines[j].strip() != "failures:":
                j += 1
            found.append((f"Test failed: {m.group(1)}", "\n".join(lines[i + 1 : j])))
            i = j
            continue
        # A panic outside a test (a test binary crashing, or a build script).
        if re.match(r"^thread '.*' panicked at", line) and not any(t.startswith("Test failed") for t, _ in found):
            found.append(("Panic", "\n".join(lines[i : i + 25])))
        i += 1
    return found


def main() -> None:
    folder = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "ci-logs")
    logs = sorted(folder.glob("*.log"), key=lambda p: p.stat().st_mtime)
    if not logs:
        note("No log", f"nothing in {folder}")
        return
    log = logs[-1]
    lines = log.read_text(encoding="utf-8", errors="replace").splitlines()
    found = problems(lines)
    summary = [l for l in lines if l.startswith("test result: FAILED") or l.startswith("error: test failed")]
    if summary:
        found.append(("Test summary", "\n".join(summary)))
    if not found:
        found = [(f"{log.stem} failed (end of its output)", "\n".join(lines[-80:]))]
    for title, body in found[:MAX_NOTES]:
        note(f"{log.stem}: {title}", body)
    if len(found) > MAX_NOTES:
        note(f"{log.stem}: more", f"{len(found) - MAX_NOTES} more problems in the full log")


if __name__ == "__main__":
    main()
