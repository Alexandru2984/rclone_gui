#!/usr/bin/env python3
"""Generate deterministic Flatpak Cargo sources from the committed lockfile."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
LOCKFILE = ROOT / "Cargo.lock"
OUTPUT = ROOT / "packaging/flatpak/cargo-sources.json"
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
SAFE_NAME = re.compile(r"[A-Za-z0-9_-]+\Z")
SAFE_VERSION = re.compile(r"[A-Za-z0-9.+-]+\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")


def generate() -> str:
    with LOCKFILE.open("rb") as handle:
        lock = tomllib.load(handle)

    sources: list[dict[str, str]] = []
    for package in lock["package"]:
        source = package.get("source")
        if source is None:
            continue
        if source != CRATES_IO:
            raise ValueError(f"unsupported Cargo source: {source}")

        name = package["name"]
        version = package["version"]
        checksum = package.get("checksum", "")
        if not SAFE_NAME.fullmatch(name) or not SAFE_VERSION.fullmatch(version):
            raise ValueError(f"unsafe crate identity: {name} {version}")
        if not SHA256.fullmatch(checksum):
            raise ValueError(f"missing or invalid checksum for {name} {version}")

        destination = f"cargo/vendor/{name}-{version}"
        sources.extend(
            [
                {
                    "type": "archive",
                    "archive-type": "tar-gzip",
                    "url": (
                        f"https://static.crates.io/crates/{name}/"
                        f"{name}-{version}.crate"
                    ),
                    "sha256": checksum,
                    "dest": destination,
                },
                {
                    "type": "inline",
                    "contents": json.dumps({"package": checksum, "files": {}}),
                    "dest": destination,
                    "dest-filename": ".cargo-checksum.json",
                },
            ]
        )

    sources.append(
        {
            "type": "inline",
            "contents": (
                '[source.vendored-sources]\n'
                'directory = "cargo/vendor"\n\n'
                '[source.crates-io]\n'
                'replace-with = "vendored-sources"\n'
            ),
            "dest": "cargo",
            "dest-filename": "config",
        }
    )
    return json.dumps(sources, indent=4) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check", action="store_true", help="fail unless cargo-sources.json is current"
    )
    args = parser.parse_args()

    try:
        generated = generate()
    except (KeyError, OSError, tomllib.TOMLDecodeError, ValueError) as error:
        print(f"cargo source generation failed: {error}", file=sys.stderr)
        return 1

    if args.check:
        try:
            current = OUTPUT.read_text(encoding="utf-8")
        except OSError:
            current = ""
        if current != generated:
            print(
                "packaging/flatpak/cargo-sources.json is stale; regenerate it",
                file=sys.stderr,
            )
            return 1
        return 0

    OUTPUT.write_text(generated, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
