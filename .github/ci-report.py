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
ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")


def escape(text: str) -> str:
    return text.replace("%", "%25").replace("\r", "").replace("\n", "%0A")


def note(title: str, body: str, keep_end: bool = False) -> None:
    body = body.strip()
    if len(body) > MAX_CHARS:
        body = ("…\n" + body[-MAX_CHARS:]) if keep_end else (body[:MAX_CHARS] + "\n…")
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
        # A failed test's captured output (tests run with output captured).
        m = re.match(r"^---- (.+) stdout ----$", line)
        if m:
            j = i + 1
            while j < len(lines) and not lines[j].startswith("---- ") and lines[j].strip() != "failures:":
                j += 1
            found.append((f"Test failed: {m.group(1)}", "\n".join(lines[i + 1 : j])))
            i = j
            continue
        # A panic printed as it happened (tests run with --nocapture).
        m = re.match(r"^thread '(.*?)'.* panicked at", line)
        if m:
            j = i + 1
            while j < len(lines) and j < i + 30 and not lines[j].startswith("test ") and not lines[j].startswith("note: run with"):
                j += 1
            block = lines[i:j]
            # A test that expects a panic (it still passed).
            if "ok" not in (l.strip() for l in block):
                found.append((f"Panic in {m.group(1)}", "\n".join(block)))
            i = j
            continue
        i += 1
    return found


def crashed_in(lines: list[str]) -> str | None:
    """With --test-threads=1, the test running when the process died."""
    if any(l.startswith("test result:") for l in lines):
        return None
    for l in reversed(lines):
        m = re.match(r"^test (\S+) \.\.\. ?(\S*)", l)
        if m:
            return None if m.group(2) in ("ok", "FAILED", "ignored") else m.group(1)
    return None


def main() -> None:
    folder = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "ci-logs")
    logs = sorted(folder.glob("*.log"), key=lambda p: p.stat().st_mtime)
    if not logs:
        note("No log", f"nothing in {folder}")
        return
    log = logs[-1]
    text = ANSI.sub("", log.read_text(encoding="utf-8", errors="replace"))
    lines = text.splitlines()
    found = problems(lines)
    summary = [l for l in lines if l.startswith("test result:") or "process didn't exit successfully" in l or l.startswith("error: test failed")]
    failed = [l.strip() for l in lines if l.endswith("... FAILED")]
    if failed or summary:
        found.append(("Summary", "\n".join(failed + summary)))
    crash = crashed_in(lines)
    if crash:
        found.insert(0, (f"Crashed while running {crash}", "\n".join(lines[-40:])))
    for title, body in found[: MAX_NOTES - 1]:
        note(f"{log.stem}: {title}", body)
    if len(found) > MAX_NOTES - 1:
        note(f"{log.stem}: more", f"{len(found) - MAX_NOTES + 1} more problems in the full log")
    # The end of the output, always.
    note(f"{log.stem}: end of the output", "\n".join(lines[-120:]), keep_end=True)


if __name__ == "__main__":
    main()
