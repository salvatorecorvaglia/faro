"""Check manifest versions, optionally against a release tag."""
import argparse
import json
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--tag", help="Release ref, including refs/tags/v prefix")
args = parser.parse_args()

versions = {
    "package.json": json.loads((ROOT / "package.json").read_text())["version"],
    "tauri.conf.json": json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())["version"],
    "Cargo.toml": tomllib.loads((ROOT / "src-tauri/Cargo.toml").read_text())["package"]["version"],
}
if (
    any(not isinstance(value, str) or not value for value in versions.values())
    or len(set(versions.values())) != 1
):
    raise SystemExit(f"Version mismatch: {versions}")
version = next(iter(versions.values()))
if args.tag is not None:
    if not args.tag.startswith("refs/tags/v") or args.tag != f"refs/tags/v{version}":
        raise SystemExit(f"Release ref {args.tag!r} must match refs/tags/v{version}")
print(f"Versions agree: {version}")
