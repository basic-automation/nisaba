#!/usr/bin/env python3
"""Assemble the Tauri updater manifest (latest.json) from a release's assets.

The Tauri CLI never writes latest.json, so the release workflow builds it
here from the .sig files the bundler produced (createUpdaterArtifacts).
Usage:

    build-latest-json.py <assets-dir> <tag> <owner/repo>

<assets-dir> holds the downloaded release assets (bundles + their .sig
files). Writes <assets-dir>/latest.json. Exits non-zero when no signed
updater artifact is found — a release nobody can update to must not ship
silently.
"""

import json
import os
import sys
import urllib.parse
from datetime import datetime, timezone


def entry(assets_dir, repo, tag, asset):
    sig_path = os.path.join(assets_dir, asset + ".sig")
    if not os.path.exists(sig_path):
        return None
    with open(sig_path, encoding="utf-8") as f:
        signature = f.read().strip()
    url = "https://github.com/{}/releases/download/{}/{}".format(
        repo, tag, urllib.parse.quote(asset)
    )
    return {"signature": signature, "url": url}


def platform_keys(asset):
    if asset.endswith(".AppImage"):
        return ["linux-x86_64", "linux-x86_64-appimage"]
    if asset.endswith(".deb"):
        return ["linux-x86_64-deb"]
    if asset.endswith(".rpm"):
        return ["linux-x86_64-rpm"]
    if asset.endswith(".app.tar.gz"):
        # The macOS bundle is a universal binary; both arch keys point at it.
        return ["darwin-aarch64", "darwin-x86_64"]
    if asset.endswith("-setup.exe"):
        return ["windows-x86_64", "windows-x86_64-nsis"]
    if asset.endswith(".msi"):
        return ["windows-x86_64-msi"]
    return []


def main():
    assets_dir, tag, repo = sys.argv[1], sys.argv[2], sys.argv[3]
    version = tag[1:] if tag.startswith("v") else tag

    platforms = {}
    for name in sorted(os.listdir(assets_dir)):
        if name.endswith(".sig") or name == "latest.json":
            continue
        keys = platform_keys(name)
        if not keys:
            continue
        signed = entry(assets_dir, repo, tag, name)
        if signed is None:
            continue
        for key in keys:
            platforms[key] = signed

    if not platforms:
        print(
            "no signed updater artifacts found — refusing to publish a "
            "release without latest.json (are the TAURI_SIGNING_* secrets set?)",
            file=sys.stderr,
        )
        return 1

    manifest = {
        "version": version,
        "notes": "",
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }
    out = os.path.join(assets_dir, "latest.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
    print("latest.json platforms:", ", ".join(sorted(platforms)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
