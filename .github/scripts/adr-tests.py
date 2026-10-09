#!/usr/bin/env python3
"""Report ADR claims from libtest's registered tests, not source-text matches."""

import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def report(root, listing, ignored=""):
    ignored_names = {
        line.removesuffix(": test") for line in ignored.splitlines() if line.endswith(": test")
    }
    records = {p.name[:4]: p for p in (root / "docs/adr").glob("[0-9][0-9][0-9][0-9]-*.md")}
    tests = {number: [] for number in records}
    errors = [] if records else ["No ADR records found"]
    for line in listing.splitlines():
        if not line.endswith(": test"):
            continue
        name = line.removesuffix(": test")
        claim = name.split("::")[-1]
        if not claim.startswith("adr_"):
            continue
        if name in ignored_names:
            errors.append(f"Ignored ADR test is not evidence: {name}")
            continue
        match = re.fullmatch(r"adr_(\d{4})_([a-z][a-z0-9_]+)", claim)
        if not match or match[1] not in records:
            errors.append(f"Unknown or malformed ADR test: {name}")
        else:
            tests[match[1]].append(name)
    for number, path in sorted(records.items()):
        text = path.read_text()
        header = text.split("## ", 1)[0]
        implemented = bool(re.search(r"\bImplemented\b", header))
        exemption = re.search(r"^Test exemption: (.+)$", text, re.MULTILINE)
        names = sorted(tests[number])
        if names:
            print(f"ADR {number}: {len(names)} registered claim tests")
            for name in names:
                print(f"  {name}")
            if exemption:
                errors.append(f"ADR {number}: stale exemption; tests now exist")
        elif exemption:
            print(f"ADR {number}: NO RUST CLAIM TEST — {exemption[1]}")
        elif implemented:
            print(f"ADR {number}: MISSING TESTS")
            errors.append(
                f"ADR {number}: implemented but has no registered claim test or exemption"
            )
        else:
            print(f"ADR {number}: no tests (principle or proposal, not marked Implemented)")
    for error in errors:
        print(f"ERROR: {error}")
    return not errors


def main():
    # A separate CI step runs these tests; listing is not evidence of a pass.
    result = subprocess.run(
        ["cargo", "test", "--release", "--locked", "--", "--list"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        check=True,
    )
    ignored = subprocess.run(
        ["cargo", "test", "--release", "--locked", "--", "--ignored", "--list"],
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        check=True,
    )
    raise SystemExit(0 if report(ROOT, result.stdout, ignored.stdout) else 1)


if __name__ == "__main__":
    main()
