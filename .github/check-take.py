#!/usr/bin/env python3
"""Checks CI's test recording: the newest take has a Program file with
pictures in it, and nothing went wrong except this machine being too slow
(GitHub's machines are shared and virtual, and sometimes drop a frame).

Usage: check-take.py LOG RECORDINGS_FOLDER EXIT_STATUS
"""

import pathlib
import sys

TOO_SLOW = ("couldn't keep up", "fell behind", "overloaded", "arrived late")
PREFIX = "take finished with problems: "


def fail(message: str) -> None:
    print(f"::error title=Test recording::{message}")
    sys.exit(1)


def main() -> None:
    log, folder, status = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), int(sys.argv[3])
    takes = sorted((p for p in folder.glob("*") if p.is_dir()), key=lambda p: p.stat().st_mtime)
    if not takes:
        fail(f"no take in {folder}")
    program = takes[-1] / "Program.mp4"
    if not program.is_file():
        fail(f"no Program.mp4 in {takes[-1]}")
    size = program.stat().st_size
    if size < 200_000:
        fail(f"Program.mp4 is only {size} bytes")
    if status != 0:
        text = log.read_text(encoding="utf-8", errors="replace")
        errors = [l.strip()[len("error: "):] for l in text.splitlines() if l.strip().startswith("error: ")]
        if not errors:
            fail(f"Cutaway stopped with status {status} and no error in its log")
        problems = []
        for e in errors:
            problems += e[len(PREFIX):].split("; ") if e.startswith(PREFIX) else [e]
        real = [p for p in problems if not any(s in p for s in TOO_SLOW)]
        if real:
            fail("; ".join(real))
        print(f"::warning title=Test recording::fine apart from this machine being too slow: {'; '.join(problems)}")
    print(f"Test recording OK: {program} ({size} bytes)")


if __name__ == "__main__":
    main()
