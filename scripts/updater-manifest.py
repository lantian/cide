#!/usr/bin/env python3
"""Write `latest.json`, the manifest `tauri-plugin-updater` reads, from a release's assets.

Run by `release.yml`'s `publish` job over the flattened `dist/`, after every artefact is in it
and before `gh release create`, so the manifest is one of the release's assets and
`releases/latest/download/latest.json` (the endpoint in tauri.conf.json) resolves to it.

    scripts/updater-manifest.py --dir dist --version 0.11.0 --repo lantian/cide [--notes-file f]

It reads the `.sig` files `cargo tauri build` wrote beside each updater artefact — the AppImage
itself on Linux, `<name>_<version>_<arch>.app.tar.gz` on macOS (renamed so by the macos job) —
and maps each to the platform key the plugin looks up: `linux-x86_64`, `darwin-aarch64`, … The
signature is the `.sig` file's content verbatim; that is the format the plugin verifies.

A Python script and not an `xtask` subcommand because the `publish` job has no Rust toolchain
and never checks the repository out beyond this file: building xtask there would add minutes
and a cache to a job that otherwise only moves files. Stdlib only, for the same reason.

Refuses (exit 1) when it finds no signed artefact at all: a release with a `latest.json` naming
no platform would tell every installed copy "nothing for you" — the same silence as no manifest,
but looking deliberate.

`--self-test` runs the name parsing against fixed examples and exits.
"""

from __future__ import annotations

import argparse
import datetime
import json
import pathlib
import re
import sys
import urllib.parse

# The bundler's spelling of an architecture → the plugin's (`updater_arch()` in the plugin).
ARCH = {
    "amd64": "x86_64",
    "x86_64": "x86_64",
    "aarch64": "aarch64",
    "arm64": "aarch64",
    "i386": "i686",
    "i686": "i686",
    "armhf": "armv7",
    "armv7": "armv7",
}

# `cide_0.11.0_amd64.AppImage` / `cide_0.11.0_aarch64.app.tar.gz`. The product name may carry
# underscores of its own, so the version and arch are taken from the right.
PATTERNS = [
    (re.compile(r"^.+_(?P<version>[^_]+)_(?P<arch>[^_]+)\.AppImage$"), "linux"),
    (re.compile(r"^.+_(?P<version>[^_]+)_(?P<arch>[^_]+)\.app\.tar\.gz$"), "darwin"),
]


def platform_of(name: str) -> tuple[str, str] | None:
    """(platform key, version) for an updater artefact's file name, or None."""
    for pattern, os_name in PATTERNS:
        m = pattern.match(name)
        if m is None:
            continue
        arch = ARCH.get(m.group("arch"))
        if arch is None:
            raise SystemExit(f"updater-manifest: unknown architecture in {name}")
        return f"{os_name}-{arch}", m.group("version")
    return None


def build(directory: pathlib.Path, version: str, repo: str, notes: str | None) -> dict:
    platforms: dict[str, dict[str, str]] = {}
    for sig in sorted(directory.glob("*.sig")):
        artefact = sig.with_suffix("")  # drops only the trailing `.sig`
        found = platform_of(artefact.name)
        if found is None:
            print(f"updater-manifest: ignoring {sig.name} (not an updater artefact)", file=sys.stderr)
            continue
        key, named_version = found
        if named_version != version:
            raise SystemExit(
                f"updater-manifest: {artefact.name} says {named_version}, the release is {version}"
            )
        if not artefact.is_file():
            raise SystemExit(f"updater-manifest: {sig.name} has no {artefact.name} beside it")
        if key in platforms:
            raise SystemExit(f"updater-manifest: two artefacts for {key}")
        platforms[key] = {
            "signature": sig.read_text().strip(),
            "url": f"https://github.com/{repo}/releases/download/v{version}/"
            + urllib.parse.quote(artefact.name),
        }
    if not platforms:
        raise SystemExit(
            "updater-manifest: no signed updater artefact in "
            f"{directory} — was TAURI_SIGNING_PRIVATE_KEY set for the build jobs?"
        )
    manifest: dict = {
        "version": version,
        "pub_date": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "platforms": platforms,
    }
    if notes:
        manifest["notes"] = notes
    return manifest


def self_test() -> None:
    assert platform_of("cide_0.11.0_amd64.AppImage") == ("linux-x86_64", "0.11.0")
    assert platform_of("cide_0.11.0_aarch64.app.tar.gz") == ("darwin-aarch64", "0.11.0")
    assert platform_of("my_app_1.2.3_arm64.app.tar.gz") == ("darwin-aarch64", "1.2.3")
    assert platform_of("cide_0.11.0_aarch64.dmg") is None
    assert platform_of("cide-0.11.0-linux-x86_64.tar.gz") is None
    assert platform_of("SHA256SUMS") is None
    print("updater-manifest: self-test ok")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dir", type=pathlib.Path)
    parser.add_argument("--version")
    parser.add_argument("--repo")
    parser.add_argument("--notes-file", type=pathlib.Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.dir is None or args.version is None or args.repo is None:
        parser.error("--dir, --version and --repo are required")
    notes = args.notes_file.read_text().strip() if args.notes_file else None
    manifest = build(args.dir, args.version.lstrip("v"), args.repo, notes)
    out = args.dir / "latest.json"
    out.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"updater-manifest: wrote {out} for {', '.join(sorted(manifest['platforms']))}")


if __name__ == "__main__":
    main()
