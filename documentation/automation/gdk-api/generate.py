#!/usr/bin/env python3
"""Generate GDK API reference data from the UniFFI surface in goose-sdk.

`crates/goose-sdk/src/bindings.rs` is the single source of truth for the Rust,
Python, and Kotlin GDK APIs, so the docs are derived from it instead of being
written by hand. Output is `documentation/src/data/gdk-api.json`, holding one
entry per GDK release series, consumed by the GdkApiReference component.

Usage:
    python3 documentation/automation/gdk-api/generate.py [--check]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
BINDINGS = REPO_ROOT / "crates/goose-sdk/src/bindings.rs"
CARGO_TOML = REPO_ROOT / "crates/goose-sdk/Cargo.toml"
OUT_FILE = REPO_ROOT / "documentation/src/data/gdk-api.json"


def crate_version() -> str:
    match = re.search(r'^version\s*=\s*"([^"]+)"', CARGO_TOML.read_text(), re.MULTILINE)
    if not match:
        sys.exit(f"could not read version from {CARGO_TOML}")
    return match.group(1)


def doc_version(version: str) -> str:
    """Docs are versioned per release series, e.g. 0.1.0-alpha.6 -> 0.1."""
    major, minor = version.split(".")[:2]
    return f"{major}.{minor}"


@dataclass
class Param:
    name: str
    type: str
    default: str | None = None
    docs: str = ""


@dataclass
class Func:
    name: str
    docs: str = ""
    params: list[Param] = field(default_factory=list)
    returns: str | None = None
    throws: str | None = None
    is_async: bool = False


@dataclass
class Item:
    name: str
    kind: str
    docs: str = ""
    fields: list[Param] = field(default_factory=list)
    variants: list[dict] = field(default_factory=list)
    methods: list[Func] = field(default_factory=list)


def split_top_level(text: str, sep: str = ",") -> list[str]:
    parts, depth, current = [], 0, ""
    for char in text:
        if char in "<([{":
            depth += 1
        elif char in ">)]}":
            depth -= 1
        if char == sep and depth == 0:
            parts.append(current)
            current = ""
        else:
            current += char
    if current.strip():
        parts.append(current)
    return [part.strip() for part in parts if part.strip()]


def unwrap(type_text: str, wrapper: str) -> str | None:
    match = re.fullmatch(rf"{wrapper}\s*<(.+)>", type_text.strip(), re.DOTALL)
    return match.group(1).strip() if match else None


def clean_type(type_text: str) -> str:
    type_text = re.sub(r"\s+", " ", type_text).strip()
    while True:
        inner = unwrap(type_text, r"(?:std::sync::)?Arc") or unwrap(type_text, r"Box<\s*dyn")
        if inner is None:
            inner = unwrap(type_text, "Box")
        if inner is None:
            break
        type_text = re.sub(r"^dyn\s+", "", inner)
    return type_text


class Scanner:
    """Line scanner that pairs doc comments and attributes with the next item."""

    def __init__(self, source: str) -> None:
        self.lines = source.splitlines()
        self.index = 0
        self.docs: list[str] = []
        self.attrs: list[str] = []

    def take_docs(self) -> str:
        docs = "\n".join(self.docs).strip()
        self.docs = []
        return docs

    def block(self) -> str:
        """Consume from the current line through its balanced brace block."""
        text, depth, started = "", 0, False
        while self.index < len(self.lines):
            line = self.lines[self.index]
            self.index += 1
            text += line + "\n"
            depth += line.count("{") - line.count("}")
            started = started or "{" in line
            if started and depth <= 0:
                break
            if not started and line.rstrip().endswith(";"):
                break
        return text


def parse_fields(block: str) -> list[Param]:
    fields: list[Param] = []
    docs: list[str] = []
    default: str | None = None
    for line in block.splitlines():
        stripped = line.strip()
        if stripped.startswith("///"):
            docs.append(stripped[3:].strip())
            continue
        match = re.match(r"#\[uniffi\(default\s*=\s*(.+?)\)\]", stripped)
        if match:
            default = match.group(1).strip()
            continue
        match = re.match(r"pub\s+([a-z_0-9]+)\s*:\s*(.+?),?$", stripped)
        if match:
            fields.append(
                Param(
                    match.group(1),
                    clean_type(match.group(2)),
                    default,
                    " ".join(docs).strip(),
                )
            )
            docs, default = [], None
    return fields


def parse_variants(block: str) -> list[dict]:
    body = block[block.index("{") + 1 : block.rindex("}")]
    variants: list[dict] = []
    for chunk in split_top_level(re.sub(r"#\[[^\]]*\]", "", body)):
        chunk = chunk.strip()
        match = re.match(r"^([A-Z]\w*)\s*\{(.*)\}$", chunk, re.DOTALL)
        if match:
            fields = [
                Param(name.strip(), clean_type(type_text))
                for name, _, type_text in (
                    part.partition(":") for part in split_top_level(match.group(2))
                )
                if name.strip() and not name.strip().startswith("#")
            ]
            variants.append({"name": match.group(1), "fields": [vars(f) for f in fields]})
        elif re.fullmatch(r"[A-Z]\w*", chunk):
            variants.append({"name": chunk, "fields": []})
    return variants


def parse_arg_defaults(attrs: str) -> dict[str, str]:
    """Reads argument defaults from `#[uniffi::export(default(arg = value))]`."""
    defaults: dict[str, str] = {}
    for group in re.findall(r"default\s*\(([^()]*)\)", attrs):
        for part in split_top_level(group):
            name, _, value = part.partition("=")
            if value:
                defaults[name.strip()] = value.strip()
    return defaults


def parse_signature(signature: str, docs: str, defaults: dict[str, str] | None = None) -> Func:
    defaults = defaults or {}
    signature = re.sub(r"\s+", " ", signature).strip().rstrip("{;").strip()
    is_async = " async fn " in f" {signature} "
    match = re.search(r"fn\s+(\w+)\s*\((.*)\)\s*(?:->\s*(.+))?$", signature, re.DOTALL)
    if not match:
        return Func(name=signature, docs=docs)
    name, raw_params, raw_return = match.group(1), match.group(2), match.group(3)

    params = []
    for part in split_top_level(raw_params):
        if re.fullmatch(r"&?\s*(mut\s+)?self", part):
            continue
        param_name, _, type_text = part.partition(":")
        if type_text:
            name_text = param_name.strip()
            params.append(Param(name_text, clean_type(type_text), defaults.get(name_text)))

    returns, throws = None, None
    if raw_return:
        result = clean_type(raw_return)
        inner = unwrap(result, "Result")
        if inner:
            parts = split_top_level(inner)
            returns = clean_type(parts[0])
            throws = clean_type(parts[1]) if len(parts) > 1 else "GooseError"
        else:
            returns = result
    if returns in ("()", ""):
        returns = None
    return Func(name=name, docs=docs, params=params, returns=returns, throws=throws, is_async=is_async)


def parse_bindings(source: str) -> dict[str, list[Item] | list[Func]]:
    source = source.split("#[cfg(test)]")[0]
    scanner = Scanner(source)
    items: list[Item] = []
    functions: list[Func] = []

    while scanner.index < len(scanner.lines):
        line = scanner.lines[scanner.index]
        stripped = line.strip()

        if stripped.startswith("///"):
            scanner.docs.append(stripped[3:].strip())
            scanner.index += 1
            continue
        if stripped.startswith("#["):
            scanner.attrs.append(stripped)
            scanner.index += 1
            continue
        if not stripped or stripped.startswith("//"):
            scanner.index += 1
            scanner.docs = []
            continue

        attrs = " ".join(scanner.attrs)
        scanner.attrs = []
        docs = scanner.take_docs()
        exported = "uniffi::export" in attrs

        if "uniffi::Record" in attrs and stripped.startswith("pub struct"):
            block = scanner.block()
            name = re.search(r"pub struct\s+(\w+)", block).group(1)
            items.append(Item(name, "record", docs, fields=parse_fields(block)))
            continue
        if "uniffi::Object" in attrs and stripped.startswith("pub struct"):
            block = scanner.block()
            name = re.search(r"pub struct\s+(\w+)", block).group(1)
            items.append(Item(name, "object", docs))
            continue
        if ("uniffi::Enum" in attrs or "uniffi::Error" in attrs) and stripped.startswith("pub enum"):
            block = scanner.block()
            name = re.search(r"pub enum\s+(\w+)", block).group(1)
            kind = "error" if "uniffi::Error" in attrs else "enum"
            items.append(Item(name, kind, docs, variants=parse_variants(block)))
            continue
        if exported and stripped.startswith("pub trait"):
            block = scanner.block()
            name = re.search(r"pub trait\s+(\w+)", block).group(1)
            methods = [
                parse_signature(match, "")
                for match in re.findall(r"fn\s+\w+\s*\([^;]*?\)\s*(?:->[^;]+)?;", block)
            ]
            items.append(Item(name, "callback", docs, methods=methods))
            continue
        if exported and stripped.startswith("impl "):
            block = scanner.block()
            target = re.search(r"impl\s+(\w+)", block).group(1)
            owner = next((item for item in items if item.name == target), None)
            if owner:
                owner.methods.extend(parse_impl_methods(block))
            continue
        if exported and re.match(r"pub\s+(async\s+)?fn", stripped):
            block = scanner.block()
            functions.append(parse_signature(block.split("{")[0], docs, parse_arg_defaults(attrs)))
            continue

        scanner.index += 1

    return {"items": items, "functions": functions}


def parse_impl_methods(block: str) -> list[Func]:
    methods: list[Func] = []
    lines = block.splitlines()
    docs: list[str] = []
    index = 0
    while index < len(lines):
        stripped = lines[index].strip()
        if stripped.startswith("///"):
            docs.append(stripped[3:].strip())
            index += 1
            continue
        if re.match(r"pub\s+(async\s+)?fn", stripped):
            signature, depth = "", 0
            while index < len(lines):
                signature += lines[index] + "\n"
                depth += lines[index].count("(") - lines[index].count(")")
                if depth <= 0 and ("{" in lines[index] or ";" in lines[index]):
                    break
                index += 1
            methods.append(parse_signature(signature.split("{")[0], "\n".join(docs).strip()))
            docs = []
        elif stripped and not stripped.startswith("#"):
            docs = []
        index += 1
    return methods


def build(version: str) -> dict:
    parsed = parse_bindings(BINDINGS.read_text())
    items: list[Item] = parsed["items"]
    functions: list[Func] = parsed["functions"]

    if not items or not functions:
        sys.exit("parsed no API items; the bindings layout likely changed")

    def serialize_func(func: Func) -> dict:
        return {
            "name": func.name,
            "docs": func.docs,
            "params": [vars(param) for param in func.params],
            "returns": func.returns,
            "throws": func.throws,
            "isAsync": func.is_async,
        }

    def serialize_item(item: Item) -> dict:
        return {
            "name": item.name,
            "kind": item.kind,
            "docs": item.docs,
            "fields": [vars(field_) for field_ in item.fields],
            "variants": item.variants,
            "methods": [serialize_func(method) for method in item.methods],
        }

    return {
        "version": version,
        "docVersion": doc_version(version),
        "source": "crates/goose-sdk/src/bindings.rs",
        "functions": [serialize_func(func) for func in sorted(functions, key=lambda f: f.name)],
        "items": [serialize_item(item) for item in items],
    }


def merge(current: dict) -> dict:
    """Upserts the current release series, keeping older series newest-first."""
    existing = json.loads(OUT_FILE.read_text())["versions"] if OUT_FILE.exists() else []
    versions = [
        entry for entry in existing if entry["docVersion"] != current["docVersion"]
    ] + [current]
    versions.sort(
        key=lambda entry: [int(part) for part in entry["docVersion"].split(".")],
        reverse=True,
    )
    return {"versions": versions}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if output is stale")
    args = parser.parse_args()

    version = crate_version()
    payload = json.dumps(merge(build(version)), indent=2) + "\n"
    relative = OUT_FILE.relative_to(REPO_ROOT)

    if args.check:
        if not OUT_FILE.exists() or OUT_FILE.read_text() != payload:
            print(f"{relative} is out of date; run {Path(__file__).name}")
            return 1
        print(f"{relative} is up to date")
        return 0

    OUT_FILE.parent.mkdir(parents=True, exist_ok=True)
    OUT_FILE.write_text(payload)
    print(f"wrote {relative} for goose-sdk {version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
