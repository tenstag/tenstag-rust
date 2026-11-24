#!/usr/bin/env python3
"""Checks that every specification document and rule tenstag's code cites exists at the pinned commit.

  TENSTAG_SPEC=<a tenstag specification checkout> tools/check_spec_citations.py

The commit and documents are spec-pin.toml's [tenstag]: a citation is `tenstag specs/<document>.md` (one of the pin's
documents, the store documents in specs/stores/ among them), and every rule id the code names (TFP-001, LST-007,
HFC-010, OLM-004, SUG-041, ...) must be a heading `### <ID>` in one of those documents. tests/vectors/ must be the same
as specs/test-vectors/ at that commit.

The checkout is read with `git show <commit>:<path>`, so it may be at any commit that has the pinned one.
"""
import os
import pathlib
import re
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
PREFIXES = "TFP|TAG|TGS|TFC|CFG|LOC|LST|NAM|HFC|LCC|OLM|LMS|G4A|MSC|JAN|DMR|RML|SUG"


def show(checkout, commit, path):
    return subprocess.run(["git", "-C", checkout, "show", f"{commit}:specs/{path}"], check=True, capture_output=True,
                          text=True).stdout


def main():
    tts = os.environ.get("TENSTAG_SPEC")
    if not tts:
        print("give TENSTAG_SPEC (a tenstag specification checkout)", file=sys.stderr)
        sys.exit(2)
    pin = tomllib.loads((ROOT / "spec-pin.toml").read_text())["tenstag"]
    defined = set()
    for doc in pin["documents"]:
        defined |= set(re.findall(rf"^### ((?:{PREFIXES})-(?:\d{{3}}|Q\d+))\b", show(tts, pin["commit"], doc), re.M))

    problems, cited = [], set()
    for source in sorted((ROOT / "src").rglob("*.rs")) + sorted((ROOT / "tests").rglob("*.rs")):
        for number, line in enumerate(source.read_text().splitlines(), 1):
            where = f"{source.relative_to(ROOT)}:{number}"
            for doc in re.findall(r"specs/((?:stores/)?[a-z0-9-]+\.md)", line):
                if doc not in pin["documents"]:
                    problems.append(f"{where}: tenstag specs/{doc} is not a pinned document")
            for rule in re.findall(rf"\b((?:{PREFIXES})-\d{{3}})\b", line):
                cited.add(rule)
                if rule not in defined:
                    problems.append(f"{where}: {rule} is not defined at the pinned commit")
    for vector in sorted((ROOT / "tests" / "vectors").glob("*.json")):
        if vector.read_text() != show(tts, pin["commit"], f"test-vectors/{vector.name}"):
            problems.append(f"tests/vectors/{vector.name} differs from the specification's at {pin['commit'][:12]}")
    for problem in problems:
        print(problem)
    print(f"{len(cited)} rules cited, {len(problems)} problems, against tenstag {pin['commit'][:12]}")
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
