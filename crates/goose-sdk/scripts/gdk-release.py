#!/usr/bin/env python3

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
CONFIG = ROOT / "release-plz.toml"
SDK = "goose-sdk"


def gdk_crates() -> list[str]:
    text = CONFIG.read_text()
    package_sections = re.findall(r"(?ms)^\[\[package\]\]\s*$(.*?)(?=^\[|\Z)", text)
    crates = []
    for section in package_sections:
        name = re.search(r'^name\s*=\s*"([^"]+)"', section, re.MULTILINE)
        release = re.search(r"^release\s*=\s*true\s*$", section, re.MULTILINE)
        group = re.search(r'^version_group\s*=\s*"gdk"\s*$', section, re.MULTILINE)
        if name and release and group:
            crates.append(name.group(1))
    if not crates:
        raise SystemExit("release-plz.toml does not define any GDK crates")
    if len(crates) != len(set(crates)):
        raise SystemExit("release-plz.toml contains duplicate GDK crates")
    if SDK not in crates:
        raise SystemExit(f"release-plz.toml GDK group must contain {SDK}")
    return crates


def metadata() -> dict:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def package_version(path: Path, section: str) -> str:
    text = path.read_text()
    section_match = re.search(rf"(?ms)^\[{re.escape(section)}\]\s*$(.*?)(?=^\[|\Z)", text)
    if not section_match:
        raise SystemExit(f"missing [{section}] in {path.relative_to(ROOT)}")
    version_match = re.search(r'^version\s*=\s*"([^"]+)"', section_match.group(1), re.MULTILINE)
    if not version_match:
        raise SystemExit(f"missing version in [{section}] in {path.relative_to(ROOT)}")
    return version_match.group(1)


def python_version(rust_version: str) -> str:
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-alpha\.(\d+))?", rust_version)
    if not match:
        raise SystemExit("version must look like 0.1.0 or 0.1.0-alpha.0")
    major, minor, patch, alpha = match.groups()
    return f"{major}.{minor}.{patch}" if alpha is None else f"{major}.{minor}.{patch}a{alpha}"


def check_config() -> None:
    crates = gdk_crates()
    packages = {package["name"]: package for package in metadata()["packages"]}
    errors = []
    for crate in crates:
        if crate not in packages:
            errors.append(f"release-plz.toml GDK crate is not a workspace package: {crate}")

    workflow = ROOT / ".github/workflows/gdk-release-pr.yml"
    workflow_text = workflow.read_text()
    begin_marker = "      # BEGIN GDK CRATE PATHS\n"
    end_marker = "      # END GDK CRATE PATHS\n"
    if workflow_text.count(begin_marker) != 1 or workflow_text.count(end_marker) != 1:
        errors.append(f"{workflow.relative_to(ROOT)} must contain one GDK crate paths marker block")
    else:
        path_block = workflow_text.split(begin_marker, 1)[1].split(end_marker, 1)[0]
        expected_block = "".join(f'      - "crates/{crate}/**"\n' for crate in crates)
        if path_block != expected_block:
            errors.append(
                f"{workflow.relative_to(ROOT)} GDK crate paths do not match release-plz.toml"
            )

    positions = {crate: index for index, crate in enumerate(crates)}
    for crate in crates:
        package = packages.get(crate)
        if not package:
            continue
        for dependency in package["dependencies"]:
            dependency_name = dependency["name"]
            if dependency_name in positions and positions[dependency_name] >= positions[crate]:
                errors.append(
                    f"release-plz.toml must list {dependency_name} before dependent crate {crate}"
                )

    if errors:
        raise SystemExit("\n".join(errors))
    print(f"Validated {len(crates)} GDK crates and their publication order")


def check_version(rust_version: str) -> None:
    crates = gdk_crates()
    expected_python = python_version(rust_version)
    errors = []
    dependency_names = set(crates) - {SDK}
    for crate in crates:
        path = ROOT / "crates" / crate / "Cargo.toml"
        text = path.read_text()
        actual = package_version(path, "package")
        if actual != rust_version:
            errors.append(f"{path.relative_to(ROOT)}: expected {rust_version}, found {actual}")
        for dependency in dependency_names:
            for match in re.finditer(
                rf"(?m)^{re.escape(dependency)}\s*=\s*\{{([^}}]*)\}}", text
            ):
                version_match = re.search(r'version\s*=\s*"([^"]+)"', match.group(1))
                if not version_match:
                    errors.append(f"{path.relative_to(ROOT)}: {dependency} is missing a version requirement")
                elif version_match.group(1) != rust_version:
                    errors.append(
                        f"{path.relative_to(ROOT)}: {dependency} expected {rust_version}, "
                        f"found {version_match.group(1)}"
                    )

    pyproject = ROOT / "crates/goose-sdk/python/pyproject.toml"
    actual_python = package_version(pyproject, "project")
    if actual_python != expected_python:
        errors.append(
            f"{pyproject.relative_to(ROOT)}: expected {expected_python}, found {actual_python}"
        )
    if errors:
        raise SystemExit("\n".join(errors))
    print(f"Validated Rust/Maven version {rust_version} and Python version {expected_python}")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("crates")
    subparsers.add_parser("check-config")
    check = subparsers.add_parser("check-version")
    check.add_argument("version")
    args = parser.parse_args()

    if args.command == "crates":
        print("\n".join(gdk_crates()))
    elif args.command == "check-config":
        check_config()
    elif args.command == "check-version":
        check_version(args.version)


if __name__ == "__main__":
    main()
