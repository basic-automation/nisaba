#!/usr/bin/env python3
"""Fail if the version strings in Cargo.toml, tauri.conf.json and
frontend/package.json disagree.

Phase 6 release-engineering gate (N-10.2): the three version strings must be
bumped together; nothing checked that before. CI runs this as its own job in
.github/workflows/ci.yml.
"""

import json
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

SOURCES = {
    "Cargo.toml [workspace.package]": (
        "Cargo.toml",
        lambda root: tomllib.loads(root.read_text("utf-8"))["workspace"]["package"]["version"],
    ),
    "crates/tauri-app/tauri.conf.json": (
        "crates/tauri-app/tauri.conf.json",
        lambda root: json.loads(root.read_text("utf-8"))["version"],
    ),
    "frontend/package.json": (
        "frontend/package.json",
        lambda root: json.loads(root.read_text("utf-8"))["version"],
    ),
}


def main() -> int:
    versions = {}
    for label, (relpath, read) in SOURCES.items():
        path = REPO / relpath
        if not path.is_file():
            print(f"ERROR: {label} not found at {path}", file=sys.stderr)
            return 1
        try:
            versions[label] = str(read(path))
        except (KeyError, TypeError, tomllib.TOMLDecodeError, json.JSONDecodeError) as exc:
            print(f"ERROR: could not read version from {label}: {exc}", file=sys.stderr)
            return 1

    distinct = sorted(set(versions.values()))
    for label, version in versions.items():
        print(f"{label}: {version}")

    if len(distinct) != 1:
        print(
            f"ERROR: version strings disagree: {distinct}. "
            "Bump all three together (Cargo.toml, tauri.conf.json, frontend/package.json).",
            file=sys.stderr,
        )
        return 1

    print(f"OK: all version strings agree ({distinct[0]})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
