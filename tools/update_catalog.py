#!/usr/bin/env python3
"""Regenerate the extension catalogs from the prebuilt plugins.

The catalogs are the contract the Tanoshi Mobile Server app consumes:

  android/extensions/index.json   (fetched for the repo link / raw URL)
  tanoshi-extensions.json         (root convenience copy, same content)

Both list entries with `name`, `displayName`, `description`, `url`, `sha256`,
`size` and `ported`. `tools/catalog-meta.json` is the single source of truth for
the human fields; this script fills in `url`, `sha256` and `size` from the actual
`android/extensions/plugins/lib<name>.so` files.

Usage:
  python3 tools/update_catalog.py           # rewrite both catalogs
  python3 tools/update_catalog.py --check   # verify the catalogs are current
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from datetime import date
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
META_PATH = REPO_ROOT / "tools" / "catalog-meta.json"
MULTISRC_PATH = REPO_ROOT / "tools" / "multisrc.json"
PLUGINS_DIR = REPO_ROOT / "android" / "extensions" / "plugins"
CATALOG_PATHS = [
    REPO_ROOT / "android" / "extensions" / "index.json",
    REPO_ROOT / "tanoshi-extensions.json",
]

DEFAULT_REPOSITORY = (
    "https://raw.githubusercontent.com/spainclaro-debug/tanoshi-extension"
    "/main/android/extensions/plugins"
)


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_multisrc_entries(repository: str) -> tuple[list[dict], list[str]]:
    """One catalog entry per verified site of every multisrc theme."""
    entries: list[dict] = []
    problems: list[str] = []
    if not MULTISRC_PATH.is_file():
        return entries, problems
    manifest = json.loads(MULTISRC_PATH.read_text(encoding="utf-8"))
    for theme in manifest.get("themes", []):
        plugin_name = theme["plugin"]
        plugin = PLUGINS_DIR / f"lib{plugin_name}.so"
        if not plugin.is_file():
            problems.append(f"missing multisrc plugin: {plugin.relative_to(REPO_ROOT)}")
            continue
        sites_path = REPO_ROOT / theme["sites"]
        verified_path = REPO_ROOT / theme["verified"]
        if not sites_path.is_file() or not verified_path.is_file():
            problems.append(
                f"multisrc {theme['theme']}: missing {theme['sites']} or {theme['verified']}"
            )
            continue
        sites = {site["key"]: site for site in json.loads(sites_path.read_text(encoding="utf-8"))}
        verified = set(json.loads(verified_path.read_text(encoding="utf-8")))
        digest = sha256_of(plugin)
        size = plugin.stat().st_size
        for key, site in sites.items():
            if key not in verified:
                continue
            language = site.get("lang", "en")
            entries.append(
                {
                    "name": key,
                    "displayName": site.get("name", key),
                    "description": (
                        f"{site.get('name', key)} — {theme['theme']} theme source "
                        f"({language}) ported from Keiyoushi."
                    ),
                    "url": f"{repository}/lib{plugin_name}.so",
                    "sha256": digest,
                    "size": size,
                    "ported": True,
                }
            )
    return entries, problems


def build_catalog(meta: dict) -> dict:
    repository = meta.get("repository", DEFAULT_REPOSITORY).rstrip("/")
    extensions = []
    problems = []

    for entry in meta.get("extensions", []):
        name = entry["name"]
        # Multi-source (theme) entries share one prebuilt plugin binary: the
        # optional `plugin` field names it, otherwise the entry name is used.
        plugin_name = entry.get("plugin", name)
        plugin = PLUGINS_DIR / f"lib{plugin_name}.so"
        if not plugin.is_file():
            problems.append(f"missing plugin: {plugin.relative_to(REPO_ROOT)}")
            continue
        extensions.append(
            {
                "name": name,
                "displayName": entry.get("displayName", name),
                "description": entry.get("description", ""),
                "url": f"{repository}/lib{plugin_name}.so",
                "sha256": sha256_of(plugin),
                "size": plugin.stat().st_size,
                "ported": bool(entry.get("ported", False)),
            }
        )

    multisrc_entries, multisrc_problems = load_multisrc_entries(repository)
    problems.extend(multisrc_problems)
    known_names = {entry["name"] for entry in extensions}
    for entry in multisrc_entries:
        if entry["name"] in known_names:
            problems.append(f"duplicate catalog name: {entry['name']}")
            continue
        known_names.add(entry["name"])
        extensions.append(entry)

    catalog = {
        "version": meta.get("version", 1),
        "updated": date.today().isoformat(),
        "note": meta.get("note", ""),
        "extensions": extensions,
    }
    return catalog, problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if a catalog is missing or does not match the plugins",
    )
    args = parser.parse_args()

    meta = json.loads(META_PATH.read_text(encoding="utf-8"))
    catalog, problems = build_catalog(meta)
    if problems:
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        return 1

    payload = json.dumps(catalog, indent=2) + "\n"
    status = 0
    for path in CATALOG_PATHS:
        relative = path.relative_to(REPO_ROOT)
        if args.check:
            if not path.is_file():
                print(f"error: {relative} does not exist", file=sys.stderr)
                status = 1
                continue
            existing = json.loads(path.read_text(encoding="utf-8"))
            if existing.get("extensions") != catalog["extensions"]:
                print(
                    f"error: {relative} is out of date; run tools/update_catalog.py",
                    file=sys.stderr,
                )
                status = 1
            else:
                print(f"ok: {relative}")
            continue
        path.write_text(payload, encoding="utf-8")
        print(f"wrote {relative} ({len(catalog['extensions'])} extensions)")

    return status


if __name__ == "__main__":
    raise SystemExit(main())
