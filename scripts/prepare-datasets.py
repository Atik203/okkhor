#!/usr/bin/env python3
"""Prepare licensed and synthetic corpora for Okkhor (0.10 Phase 5).

The tool is offline and standard-library only. It reads a local directory of
operator-supplied corpus files plus a small JSON source manifest, validates
licence and provenance fields, normalizes rows (NFC, collapsed whitespace,
ZWJ/ZWNJ preserved), drops PII, deduplicates rows, splits train/eval by a
hash of `case_id`, and writes dataset manifests in schema 1 next to their data
files (verifiable with `okkhor-train-dataset --verify`).

Layout
------
    <input>/dataset-sources.json       # operator manifest, see docs
    <input>/<corpus>.tsv|.csv|.jsonl|.txt

    <out>/collection.json              # collection index + provenance
    <out>/train/<name>.jsonl           # + <name>.manifest.json
    <out>/eval/<name>.jsonl            # + <name>.manifest.json
    <out>/freq/bn-tokens.tsv           # + bn-tokens.manifest.json

Usage
-----
    python scripts/prepare-datasets.py --input <dir> --out <dir> --dry-run
    python scripts/prepare-datasets.py --input <dir> --out <dir>
    python scripts/prepare-datasets.py --out <dir> --verify

Safety
------
- No network access, no timestamps, no absolute paths in output.
- Fails closed on a missing/unknown licence, missing provenance, non-commercial
  data used for training, or contamination between train and eval.
- Never reads user data: only files named by the operator manifest.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import math
import re
import sys
import unicodedata
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any, Iterable

# Some licensed corpora carry long article fields; the default 128 KiB CSV
# limit is too small and produced a misleading parse failure on the first run.
csv.field_size_limit(16 * 1024 * 1024)

TOOL = "prepare-datasets"
TOOL_VERSION = 1
MANIFEST_SCHEMA = 1
SPEC_SCHEMA = 1
SPEC_NAME = "dataset-sources.json"
COLLECTION_NAME = "collection.json"
MAX_NOTES_CHARS = 1024
MAX_SPEC_BYTES = 64 * 1024
DATA_EXTENSIONS = ("jsonl", "txt", "csv", "tsv")
DEFAULT_EVAL_PERCENT = 10
DEFAULT_MAX_ROWS = 2_000_000
DEFAULT_MAX_FILE_MB = 2048
NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
KINDS = ("pairs", "sentences", "corrections")
ROLES = ("hash", "train", "eval")
LANGUAGE_TAGS = ("bn", "en", "roman")
SPLITS = ("train", "eval")
ZWNJ = "\u200c"
ZWJ = "\u200d"
BN_TOKEN_RE = re.compile(r"[\u0980-\u09FF\u200C\u200D]+")

# Canonical licence id -> (tier, attribution required).
# Anything absent is rejected until a recorded legal decision adds it.
LICENSES: dict[str, tuple[str, bool]] = {
    "CC0-1.0": ("permissive", False),
    "CC-BY-4.0": ("permissive", True),
    "CC-BY-3.0": ("permissive", True),
    "MIT": ("permissive", True),
    "Apache-2.0": ("permissive", True),
    "BSD-3-Clause": ("permissive", True),
    "ODC-BY-1.0": ("permissive", True),
    "CC-BY-SA-4.0": ("share-alike", True),
    "CC-BY-SA-3.0": ("share-alike", True),
    "ODbL-1.0": ("share-alike", True),
    "GFDL-1.3": ("share-alike", True),
    "CC-BY-NC-4.0": ("non-commercial", True),
    "CC-BY-NC-SA-4.0": ("non-commercial", True),
}
LICENSE_LOOKUP = {key.casefold(): key for key in LICENSES}

EMAIL_RE = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
URL_RE = re.compile(r"(?i)\b(?:https?://|www\.)\S+")
DIGIT_RUN_RE = re.compile(r"[0-9\u09E6-\u09EF]{7,}")
PHONE_RE = re.compile(
    r"(?<![0-9\u09E6-\u09EF])\+?[0-9\u09E6-\u09EF]"
    r"[0-9\u09E6-\u09EF\s().-]{6,}"
    r"[0-9\u09E6-\u09EF](?![0-9\u09E6-\u09EF])"
)


class DatasetError(Exception):
    """A fail-closed pipeline error with an operator-facing message."""


@dataclass
class PreparedSource:
    entry: dict[str, Any]
    rows: list[dict[str, str]]
    stats: dict[str, Any] = field(default_factory=dict)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def pretty_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=True, sort_keys=True, indent=2)


def read_json_bytes(path: Path, limit: int) -> tuple[Any, bytes]:
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise DatasetError(f"cannot read {path.name}: {error}") from error
    if len(raw) > limit:
        raise DatasetError(f"{path.name}: {len(raw)} bytes exceeds the {limit} byte limit")
    try:
        return json.loads(raw.decode("utf-8-sig")), raw
    except UnicodeDecodeError as error:
        raise DatasetError(f"{path.name}: not valid UTF-8 ({error.reason})") from error
    except json.JSONDecodeError as error:
        raise DatasetError(
            f"{path.name}: malformed JSON at line {error.lineno} column {error.colno}"
        ) from error


def normalize_text(value: Any) -> str:
    """NFC, strip control/format characters except ZWJ/ZWNJ, collapse space."""
    if not isinstance(value, str):
        return ""
    text = unicodedata.normalize("NFC", value)
    kept: list[str] = []
    for char in text:
        if char in (ZWJ, ZWNJ):
            kept.append(char)
            continue
        if unicodedata.category(char).startswith("C"):
            continue
        kept.append(char)
    return " ".join("".join(kept).split())


def pii_reason(text: str) -> str | None:
    if EMAIL_RE.search(text):
        return "email"
    if URL_RE.search(text):
        return "url"
    if DIGIT_RUN_RE.search(text):
        return "long-digit-run"
    phone = PHONE_RE.search(text)
    if phone and sum(ch.isdigit() for ch in phone.group(0)) >= 8:
        return "phone"
    return None


def bangla_tokens(row: dict[str, str]) -> Iterable[str]:
    text = row.get("text") or row.get("target") or ""
    for match in BN_TOKEN_RE.finditer(text):
        token = match.group(0).strip(ZWJ + ZWNJ)
        if not token:
            continue
        if not any(unicodedata.category(char) == "Lo" for char in token):
            continue
        yield token


def split_for(case_id: str, role: str, eval_percent: int) -> str:
    if role == "eval":
        return "eval"
    if role == "train":
        return "train"
    digest = hashlib.sha256(case_id.encode("utf-8")).digest()
    bucket = int.from_bytes(digest[:8], "big") % 100
    return "eval" if bucket < eval_percent else "train"


def derive_case_id(name: str, kind: str, payload: dict[str, str]) -> str:
    material = "\0".join(
        (name, kind, payload.get("input", ""), payload.get("target", payload.get("text", "")))
    )
    return sha256_bytes(material.encode("utf-8"))[:16]


def parse_spec(input_dir: Path, spec_name: str) -> tuple[dict[str, Any], bytes]:
    spec_path = input_dir / spec_name
    try:
        resolved = spec_path.resolve()
    except OSError as error:
        raise DatasetError(f"cannot resolve source manifest: {error}") from error
    if input_dir.resolve() not in resolved.parents and resolved.parent != input_dir.resolve():
        raise DatasetError("source manifest must live inside the input directory")
    if not resolved.is_file():
        raise DatasetError(f"source manifest not found: {spec_name}")
    spec, raw = read_json_bytes(resolved, MAX_SPEC_BYTES)
    if not isinstance(spec, dict):
        raise DatasetError("source manifest must be a JSON object")
    return spec, raw


def _require_text(
    value: Any, field_name: str, where: str, *, limit: int, errors: list[str]
) -> str:
    if not isinstance(value, str) or not value.strip():
        errors.append(f"{where}: `{field_name}` is required and must not be blank")
        return ""
    text = value.strip()
    if len(text) > limit:
        errors.append(f"{where}: `{field_name}` exceeds {limit} characters")
    return text


def _canonical_license(value: Any, where: str, errors: list[str]) -> tuple[str, str]:
    if not isinstance(value, str) or not value.strip():
        errors.append(f"{where}: `license` is required; use an explicit SPDX id")
        return "", ""
    canonical = LICENSE_LOOKUP.get(value.strip().casefold())
    if canonical is None:
        errors.append(
            f"{where}: license `{value.strip()}` is not on the approved list; "
            "record a legal decision before adding it"
        )
        return value.strip(), ""
    return canonical, LICENSES[canonical][0]


def _safe_relative_path(value: Any, where: str, errors: list[str]) -> str:
    if not isinstance(value, str) or not value.strip():
        errors.append(f"{where}: `path` is required")
        return ""
    text = value.strip().replace("\\", "/")
    pure = PurePosixPath(text)
    if (
        pure.is_absolute()
        or PureWindowsPath(text).is_absolute()
        or ".." in pure.parts
    ):
        errors.append(f"{where}: `path` must be relative and must not contain `..`")
        return ""
    if pure.suffix.lower().lstrip(".") not in DATA_EXTENSIONS:
        errors.append(
            f"{where}: unsupported file type `{pure.suffix}`; "
            f"expected one of {', '.join('.' + ext for ext in DATA_EXTENSIONS)}"
        )
        return ""
    return text


def validate_spec(
    spec: dict[str, Any], input_dir: Path, cli_eval_percent: int | None
) -> tuple[dict[str, Any], int]:
    errors: list[str] = []
    if spec.get("schema") != SPEC_SCHEMA:
        errors.append(f"source manifest: `schema` must be {SPEC_SCHEMA}")
    name = _require_text(spec.get("name"), "name", "source manifest", limit=64, errors=errors)
    if name and not NAME_RE.match(name):
        errors.append("source manifest: `name` must match [a-z0-9][a-z0-9-]{0,63}")
    version = _require_text(
        spec.get("version"), "version", "source manifest", limit=32, errors=errors
    )
    if version and not re.match(r"^\d+\.\d+\.\d+$", version):
        errors.append("source manifest: `version` must be MAJOR.MINOR.PATCH")
    if cli_eval_percent is not None:
        eval_percent = cli_eval_percent
    else:
        eval_percent = spec.get("eval_percent", DEFAULT_EVAL_PERCENT)
        if not isinstance(eval_percent, int):
            errors.append("source manifest: `eval_percent` must be an integer")
            eval_percent = DEFAULT_EVAL_PERCENT
    if not 1 <= eval_percent <= 50:
        errors.append("`eval_percent` must be between 1 and 50")
    eval_required = spec.get("eval_required", True)
    if not isinstance(eval_required, bool):
        errors.append("source manifest: `eval_required` must be a boolean")
        eval_required = True

    files = spec.get("files")
    if not isinstance(files, list) or not files:
        errors.append("source manifest: `files` must be a non-empty list")
        files = []
    seen_paths: set[str] = set()
    seen_names: set[str] = set()
    entries: list[dict[str, Any]] = []
    for index, raw_entry in enumerate(files):
        where = f"source manifest files[{index}]"
        if not isinstance(raw_entry, dict):
            errors.append(f"{where}: must be an object")
            continue
        rel_path = _safe_relative_path(raw_entry.get("path"), where, errors)
        if rel_path:
            if rel_path in seen_paths:
                errors.append(f"{where}: duplicate `path` {rel_path}")
            seen_paths.add(rel_path)
            target = input_dir / rel_path
            if not target.is_file():
                errors.append(f"{where}: file not found: {rel_path}")
        stem = Path(rel_path).stem if rel_path else ""
        entry_name = raw_entry.get("name", stem)
        if not isinstance(entry_name, str) or not entry_name.strip():
            errors.append(f"{where}: `name` is required")
            entry_name = stem
        entry_name = entry_name.strip().lower()
        if entry_name and not NAME_RE.match(entry_name):
            errors.append(f"{where}: `name` must match [a-z0-9][a-z0-9-]{0,63}")
        if entry_name in seen_names:
            errors.append(f"{where}: duplicate `name` {entry_name}")
        seen_names.add(entry_name)

        kind = raw_entry.get("kind")
        if kind not in KINDS:
            errors.append(f"{where}: `kind` must be one of {', '.join(KINDS)}")
        role = raw_entry.get("role", "hash")
        if role not in ROLES:
            errors.append(f"{where}: `role` must be one of {', '.join(ROLES)}")
        license_id, tier = _canonical_license(raw_entry.get("license"), where, errors)
        source = _require_text(
            raw_entry.get("source"), "source", where, limit=200, errors=errors
        )
        if source.casefold() in {"unknown", "n/a", "tbd", "none"}:
            errors.append(f"{where}: `source` must name real provenance")
        attribution = raw_entry.get("attribution", "")
        if not isinstance(attribution, str):
            errors.append(f"{where}: `attribution` must be a string")
            attribution = ""
        attribution = attribution.strip()
        if len(attribution) > 400:
            errors.append(f"{where}: `attribution` exceeds 400 characters")
        if license_id in LICENSES and LICENSES[license_id][1] and not attribution:
            errors.append(
                f"{where}: `attribution` is required for {license_id}"
            )
        if tier == "non-commercial" and role != "eval":
            errors.append(
                f"{where}: non-commercial licence {license_id} is evaluation-only; "
                "set `role` to `eval`"
            )
        languages = raw_entry.get("languages")
        if (
            not isinstance(languages, list)
            or not languages
            or any(lang not in LANGUAGE_TAGS for lang in languages)
        ):
            errors.append(
                f"{where}: `languages` must be a non-empty list of "
                f"{', '.join(LANGUAGE_TAGS)}"
            )
            languages = []
        notes = raw_entry.get("notes", "")
        if not isinstance(notes, str) or len(notes) > 300:
            errors.append(f"{where}: `notes` must be a string of at most 300 characters")
            notes = ""

        entries.append(
            {
                "path": rel_path,
                "name": entry_name,
                "kind": kind,
                "role": role,
                "license": license_id,
                "tier": tier,
                "source": source,
                "attribution": attribution,
                "languages": list(languages),
                "notes": notes,
            }
        )

    frequency = spec.get("frequency", {})
    if frequency and not isinstance(frequency, dict):
        errors.append("source manifest: `frequency` must be an object")
        frequency = {}
    freq_license = ""
    freq_tier = ""
    freq_source = ""
    freq_attribution = ""
    if frequency:
        freq_license, freq_tier = _canonical_license(
            frequency.get("license"), "source manifest frequency", errors
        )
        freq_source = _require_text(
            frequency.get("source"),
            "source",
            "source manifest frequency",
            limit=200,
            errors=errors,
        )
        freq_attribution = frequency.get("attribution", "")
        if not isinstance(freq_attribution, str):
            errors.append("source manifest frequency: `attribution` must be a string")
            freq_attribution = ""
        freq_attribution = freq_attribution.strip()
        if len(freq_attribution) > 400:
            errors.append("source manifest frequency: `attribution` exceeds 400 characters")
        if freq_license in LICENSES and LICENSES[freq_license][1] and not freq_attribution:
            errors.append(
                f"source manifest frequency: `attribution` is required for {freq_license}"
            )
        if freq_tier == "non-commercial":
            errors.append(
                "source manifest frequency: non-commercial data cannot produce a "
                "frequency prior"
            )

    if errors:
        raise DatasetError("source manifest validation failed:\n- " + "\n- ".join(errors))

    return (
        {
            "name": name,
            "version": version,
            "eval_percent": eval_percent,
            "eval_required": eval_required,
            "files": entries,
            "frequency": {
                "license": freq_license,
                "source": freq_source,
                "attribution": freq_attribution,
            }
            if frequency
            else None,
        },
        eval_percent,
    )


def decode_text(path: Path, max_bytes: int) -> str:
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise DatasetError(f"cannot read {path.name}: {error}") from error
    if len(raw) > max_bytes:
        raise DatasetError(
            f"{path.name}: {len(raw)} bytes exceeds the {max_bytes} byte limit"
        )
    if b"\x00" in raw:
        raise DatasetError(f"{path.name}: contains NUL bytes; not a text corpus")
    try:
        return raw.decode("utf-8-sig")
    except UnicodeDecodeError as error:
        raise DatasetError(
            f"{path.name}: not valid UTF-8 at byte {error.start} ({error.reason})"
        ) from error


def _first_str(record: dict[str, Any], keys: tuple[str, ...]) -> Any:
    for key in keys:
        if key in record and record[key] is not None:
            return record[key]
    return None


def parse_records(path: Path, text: str, max_rows: int) -> list[dict[str, Any]]:
    suffix = path.suffix.lower().lstrip(".")
    records: list[dict[str, Any]] = []
    if suffix == "jsonl":
        for line_number, line in enumerate(text.splitlines(), 1):
            if not line.strip():
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError as error:
                raise DatasetError(
                    f"{path.name}:{line_number}: malformed JSON ({error.msg})"
                ) from error
            if not isinstance(value, dict):
                raise DatasetError(f"{path.name}:{line_number}: row must be a JSON object")
            records.append(value)
            if len(records) > max_rows:
                raise DatasetError(f"{path.name}: more than {max_rows} rows")
        return records
    if suffix == "txt":
        for line in text.splitlines():
            if line.strip():
                records.append({"text": line})
        if len(records) > max_rows:
            raise DatasetError(f"{path.name}: more than {max_rows} rows")
        return records

    delimiter = "\t" if suffix == "tsv" else ","
    reader = csv.DictReader(io.StringIO(text), delimiter=delimiter)
    if not reader.fieldnames:
        raise DatasetError(f"{path.name}: missing header row")
    header = {name.strip().casefold() for name in reader.fieldnames if name}
    if not header & {"text", "input", "roman", "sentence"}:
        raise DatasetError(
            f"{path.name}: header must include text/input/roman/sentence columns"
        )
    for row_number, raw in enumerate(reader, 2):
        record = {
            (key.strip().casefold() if key else ""): value
            for key, value in raw.items()
            if key is not None and value is not None and str(value).strip()
        }
        if record:
            records.append(record)
            if len(records) > max_rows:
                raise DatasetError(f"{path.name}: more than {max_rows} rows")
    return records


def prepare_source(
    entry: dict[str, Any],
    input_dir: Path,
    max_rows: int,
    max_bytes: int,
) -> PreparedSource:
    path = input_dir / entry["path"]
    text = decode_text(path, max_bytes)
    records = parse_records(path, text, max_rows)
    stats = {
        "rows_in": len(records),
        "rows_out": 0,
        "duplicates": 0,
        "cross_source_duplicates": 0,
        "dropped_empty": 0,
        "dropped_pii": 0,
    }
    rows: list[dict[str, str]] = []
    seen_payloads: set[str] = set()
    seen_case_ids: dict[str, str] = {}

    for record in records:
        case_id = record.get("case_id")
        if case_id is not None and not isinstance(case_id, (str, int)):
            raise DatasetError(f"{path.name}: case_id must be a string or integer")
        if case_id is not None:
            case_id = str(case_id).strip()
            if not case_id or len(case_id) > 128 or any(ord(ch) < 32 for ch in case_id):
                raise DatasetError(f"{path.name}: invalid case_id {case_id!r}")
        language = _first_str(record, ("language", "lang"))
        language = language.strip() if isinstance(language, str) else ""
        if language and language not in LANGUAGE_TAGS:
            raise DatasetError(
                f"{path.name}: unknown language tag {language!r} "
                f"(expected {', '.join(LANGUAGE_TAGS)})"
            )
        if not language:
            language = "bn" if "bn" in entry["languages"] else entry["languages"][0]

        if entry["kind"] in ("pairs", "corrections"):
            payload = {
                "input": normalize_text(_first_str(record, ("input", "roman", "romanized"))),
                "target": normalize_text(
                    _first_str(record, ("target", "bangla", "output", "corrected"))
                ),
            }
            if not payload["input"] or not payload["target"]:
                stats["dropped_empty"] += 1
                continue
            checked = payload["input"] + "\n" + payload["target"]
        else:
            payload = {"text": normalize_text(_first_str(record, ("text", "sentence")))}
            if not payload["text"]:
                stats["dropped_empty"] += 1
                continue
            checked = payload["text"]

        if pii_reason(checked):
            stats["dropped_pii"] += 1
            continue

        key = canonical_json(payload)
        if key in seen_payloads:
            stats["duplicates"] += 1
            continue
        if not case_id:
            case_id = derive_case_id(entry["name"], entry["kind"], payload)
        previous = seen_case_ids.get(case_id)
        if previous is not None and previous != key:
            raise DatasetError(
                f"{path.name}: case_id {case_id!r} is reused with different content"
            )
        seen_payloads.add(key)
        seen_case_ids[case_id] = key
        row = {"case_id": case_id, **payload, "language": language}
        rows.append(row)

    if not rows:
        raise DatasetError(f"{path.name}: no usable rows after normalization")
    stats["rows_out"] = len(rows)
    stats["input_sha256"] = sha256_bytes(path.read_bytes())
    stats["input_bytes"] = path.stat().st_size
    rows.sort(key=lambda row: row["case_id"])
    return PreparedSource(entry=entry, rows=rows, stats=stats)


def frequency_payload(
    counter: Counter[str], min_count: int, top_tokens: int
) -> tuple[bytes, dict[str, Any]]:
    total = sum(counter.values())
    if total == 0:
        raise DatasetError("no Bangla tokens to build a frequency prior")
    items = [(token, count) for token, count in counter.items() if count >= min_count]
    items.sort(key=lambda pair: (-pair[1], pair[0]))
    if top_tokens:
        items = items[:top_tokens]
    buckets: Counter[str] = Counter()
    lines = ["token\tcount\tper_million\tzipf\tbucket"]
    for token, count in items:
        per_million = count * 1_000_000.0 / total
        zipf = round(math.log10(per_million), 2)
        bucket = int(math.floor(zipf))
        lines.append(f"{token}\t{count}\t{per_million:.6f}\t{zipf:.2f}\t{bucket}")
        buckets[str(bucket)] += 1
    covered = sum(count for _, count in items[:1000])
    stats = {
        "tokens_total": total,
        "tokens_distinct": len(counter),
        "tokens_emitted": len(items),
        "min_count": min_count,
        "top_tokens": top_tokens,
        "coverage_top_1000": round(covered / total, 6),
        "zipf_buckets": {key: buckets[key] for key in sorted(buckets, key=int)},
    }
    data = ("\n".join(lines) + "\n").encode("utf-8")
    return data, stats


def manifest_document(
    name: str,
    version: str,
    license_id: str,
    source: str,
    data: bytes,
    rows: int,
    languages: list[str],
    split: str,
    notes: str,
) -> dict[str, Any]:
    document = {
        "schema": MANIFEST_SCHEMA,
        "name": name,
        "version": version,
        "license": license_id,
        "source": source,
        "sha256": sha256_bytes(data),
        "rows": rows,
        "languages": languages,
        "split": split,
        "notes": notes,
    }
    if len(notes) > MAX_NOTES_CHARS:
        raise DatasetError(f"manifest notes for {name} exceed {MAX_NOTES_CHARS} characters")
    return document


def _recorded_spec_path(spec: str, input_dir: Path) -> str:
    """Record a relative spec path; never leak an absolute workstation path."""
    path = Path(spec)
    if path.is_absolute():
        try:
            path = path.resolve().relative_to(input_dir)
        except ValueError:
            return path.name
    return path.as_posix()


def build_plan(
    args: argparse.Namespace,
) -> tuple[dict[str, Any], list[tuple[str, bytes]], int, int]:
    input_dir = Path(args.input).resolve()
    if not input_dir.is_dir():
        raise DatasetError(f"input directory not found: {args.input}")
    out_dir = Path(args.out).resolve()
    if out_dir == input_dir or input_dir in out_dir.parents or out_dir in input_dir.parents:
        raise DatasetError("input and output directories must be disjoint")

    spec, spec_raw = parse_spec(input_dir, args.spec)
    spec, eval_percent = validate_spec(spec, input_dir, args.eval_percent)
    max_bytes = args.max_file_mb * 1024 * 1024

    prepared: list[PreparedSource] = []
    for entry in spec["files"]:
        prepared.append(prepare_source(entry, input_dir, args.max_rows, max_bytes))

    # Exact normalized duplicates across sources are dropped after the first
    # source (spec order), so the same row can never appear in two splits.
    seen_payloads: dict[str, str] = {}
    for source in prepared:
        kept: list[dict[str, str]] = []
        for row in source.rows:
            payload = {key: row[key] for key in ("input", "target", "text") if key in row}
            key = canonical_json(payload)
            owner = seen_payloads.get(key)
            if owner is not None:
                source.stats["cross_source_duplicates"] += 1
                source.stats["rows_out"] -= 1
                continue
            seen_payloads[key] = source.entry["path"]
            kept.append(row)
        source.rows = kept
        if not kept:
            raise DatasetError(
                f"{source.entry['path']}: every row duplicates an earlier source; "
                "remove the redundant file or fix the ordering"
            )

    owners: dict[str, str] = {}
    for source in prepared:
        for row in source.rows:
            case_id = row["case_id"]
            owner = owners.get(case_id)
            if owner is not None and owner != source.entry["path"]:
                raise DatasetError(
                    f"contamination: case_id {case_id!r} appears in both "
                    f"{owner} and {source.entry['path']}; inputs must be disjoint"
                )
            owners[case_id] = source.entry["path"]

    split_of: dict[str, str] = {}
    for source in prepared:
        for row in source.rows:
            split_of[row["case_id"]] = split_for(
                row["case_id"], source.entry["role"], eval_percent
            )

    train_rows = sum(1 for value in split_of.values() if value == "train")
    eval_rows = sum(1 for value in split_of.values() if value == "eval")
    if eval_rows and not train_rows:
        raise DatasetError("split produced no training rows")
    if train_rows and not eval_rows and spec["eval_required"]:
        raise DatasetError(
            "split produced no evaluation rows; adjust eval_percent or set "
            "`eval_required` to false for a frequency-only build"
        )

    counter: Counter[str] = Counter()
    for source in prepared:
        for row in source.rows:
            if split_of[row["case_id"]] == "train":
                counter.update(bangla_tokens(row))

    planned: list[tuple[str, bytes]] = []
    outputs: dict[str, list[dict[str, Any]]] = {"train": [], "eval": [], "frequency": []}
    for source in prepared:
        entry = source.entry
        for split in SPLITS:
            rows = [row for row in source.rows if split_of[row["case_id"]] == split]
            if not rows:
                continue
            data = "".join(canonical_json(row) + "\n" for row in rows).encode("utf-8")
            name = f"{entry['name']}.{split}"
            entry_note = f"notes={entry['notes']}; " if entry["notes"] else ""
            notes = (
                f"kind={entry['kind']}; split={split}; role={entry['role']}; "
                f"rule=sha256(case_id)[0:8] mod 100 < {eval_percent}; "
                f"{entry_note}"
                f"duplicates={source.stats['duplicates']}; "
                f"cross_source_duplicates={source.stats['cross_source_duplicates']}; "
                f"dropped_pii={source.stats['dropped_pii']}; "
                f"attribution={entry['attribution'] or 'not required (CC0-1.0)'}; "
                f"tool={TOOL}/{TOOL_VERSION}"
            )
            document = manifest_document(
                name=name,
                version=spec["version"],
                license_id=entry["license"],
                source=entry["source"],
                data=data,
                rows=len(rows),
                languages=entry["languages"],
                split=split,
                notes=notes,
            )
            manifest_bytes = (
                json.dumps(document, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
            ).encode("utf-8")
            data_rel = f"{split}/{entry['name']}.jsonl"
            manifest_rel = f"{split}/{entry['name']}.manifest.json"
            planned.append((data_rel, data))
            planned.append((manifest_rel, manifest_bytes))
            outputs[split].append(
                {
                    "name": name,
                    "data": data_rel,
                    "manifest": manifest_rel,
                    "data_sha256": document["sha256"],
                    "manifest_sha256": sha256_bytes(manifest_bytes),
                    "rows": len(rows),
                    "license": entry["license"],
                    "source": entry["source"],
                    "kind": entry["kind"],
                    "role": entry["role"],
                }
            )

    frequency_stats: dict[str, Any] = {}
    if counter:
        if spec["frequency"] is None:
            raise DatasetError(
                "Bangla tokens were found but the source manifest has no "
                "`frequency` block (license/source/attribution)"
            )
        freq_data, frequency_stats = frequency_payload(
            counter, args.min_bn_count, args.top_tokens
        )
        freq = spec["frequency"]
        freq_rows = len(freq_data.splitlines()) - 1
        document = manifest_document(
            name="bn-tokens",
            version=spec["version"],
            license_id=freq["license"],
            source=freq["source"],
            data=freq_data,
            rows=freq_rows,
            languages=["bn"],
            split="train",
            notes=(
                "derived Bangla token counts from the train split only; "
                "per-million and zipf buckets; no raw corpus text; "
                f"min_count={args.min_bn_count}; top_tokens={args.top_tokens}; "
                f"source={freq['source']}; "
                f"attribution={freq['attribution'] or 'not required (CC0-1.0)'}; "
                f"tool={TOOL}/{TOOL_VERSION}"
            ),
        )
        manifest_bytes = (
            json.dumps(document, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
        ).encode("utf-8")
        planned.append(("freq/bn-tokens.tsv", freq_data))
        planned.append(("freq/bn-tokens.manifest.json", manifest_bytes))
        outputs["frequency"].append(
            {
                "name": "bn-tokens",
                "data": "freq/bn-tokens.tsv",
                "manifest": "freq/bn-tokens.manifest.json",
                "data_sha256": document["sha256"],
                "manifest_sha256": sha256_bytes(manifest_bytes),
                "rows": freq_rows,
                "license": freq["license"],
                "source": freq["source"],
                "kind": "frequency",
                "role": "train",
            }
        )
    elif spec["frequency"] is not None:
        frequency_stats = {"tokens_total": 0, "tokens_distinct": 0, "tokens_emitted": 0}

    inputs: list[dict[str, Any]] = []
    licenses: dict[str, dict[str, Any]] = {}
    attribution: list[dict[str, str]] = []
    for source in prepared:
        entry = source.entry
        stats = source.stats
        inputs.append(
            {
                "path": entry["path"],
                "name": entry["name"],
                "kind": entry["kind"],
                "role": entry["role"],
                "license": entry["license"],
                "tier": entry["tier"],
                "source": entry["source"],
                "attribution": entry["attribution"],
                "languages": entry["languages"],
                "notes": entry["notes"],
                "sha256": stats["input_sha256"],
                "bytes": stats["input_bytes"],
                "rows_in": stats["rows_in"],
                "rows_out": stats["rows_out"],
                "duplicates": stats["duplicates"],
                "cross_source_duplicates": stats["cross_source_duplicates"],
                "dropped_empty": stats["dropped_empty"],
                "dropped_pii": stats["dropped_pii"],
            }
        )
        bucket = licenses.setdefault(
            entry["license"], {"tier": entry["tier"], "files": []}
        )
        bucket["files"].append(entry["path"])
        attribution.append(
            {
                "license": entry["license"],
                "source": entry["source"],
                "attribution": entry["attribution"],
            }
        )
    if spec["frequency"] is not None and counter:
        freq = spec["frequency"]
        licenses.setdefault(freq["license"], {"tier": LICENSES[freq["license"]][0], "files": []})
        licenses[freq["license"]]["files"].append("freq/bn-tokens.tsv")
        attribution.append(
            {
                "license": freq["license"],
                "source": freq["source"],
                "attribution": freq["attribution"],
            }
        )

    collection = {
        "tool": TOOL,
        "tool_version": TOOL_VERSION,
        "schema": SPEC_SCHEMA,
        "name": spec["name"],
        "version": spec["version"],
        "eval_percent": eval_percent,
        "eval_required": spec["eval_required"],
        "split_rule": {
            "method": "sha256(case_id)[0:8] big-endian mod 100",
            "eval_below": eval_percent,
            "default_role": "hash",
            "forced_roles": {
                "train": "all rows train",
                "eval": "all rows frozen eval",
            },
        },
        "frequency": (
            {
                "min_count": args.min_bn_count,
                "top_tokens": args.top_tokens,
                "license": spec["frequency"]["license"],
                "source": spec["frequency"]["source"],
                "attribution": spec["frequency"]["attribution"],
                **frequency_stats,
            }
            if spec["frequency"] is not None and counter
            else None
        ),
        "source_manifest": {
            "path": _recorded_spec_path(args.spec, input_dir),
            "sha256": sha256_bytes(spec_raw),
        },
        "inputs": inputs,
        "outputs": outputs,
        "totals": {
            "train_rows": train_rows,
            "eval_rows": eval_rows,
            "bangla_tokens": frequency_stats.get("tokens_total", 0),
            "bangla_tokens_distinct": frequency_stats.get("tokens_distinct", 0),
        },
        "licenses": licenses,
        "attribution": attribution,
        "determinism": {
            "rows_sorted_by": "case_id",
            "json": "UTF-8, keys sorted, compact rows, two-space manifests",
            "timestamps": "none",
            "paths": "relative only",
        },
    }
    collection_bytes = (
        json.dumps(collection, ensure_ascii=False, sort_keys=True, indent=2) + "\n"
    ).encode("utf-8")
    planned.append((COLLECTION_NAME, collection_bytes))

    summary = {
        "ok": True,
        "mode": "dry-run" if args.dry_run else "build",
        "collection": dict(collection),
        "collection_sha256": sha256_bytes(collection_bytes),
        "train_rows": train_rows,
        "eval_rows": eval_rows,
        "zipf_buckets": frequency_stats.get("zipf_buckets", {}),
    }
    return summary, planned, train_rows, eval_rows


def write_plan(out_dir: Path, planned: list[tuple[str, bytes]], force: bool) -> None:
    if out_dir.exists():
        if not out_dir.is_dir():
            raise DatasetError(f"output path exists and is not a directory: {out_dir}")
        existing = [path for path in out_dir.rglob("*") if path.is_file()]
        if existing and not force:
            raise DatasetError(
                f"output directory is not empty ({len(existing)} files); "
                "pass --force to overwrite generated files"
            )
    else:
        out_dir.mkdir(parents=True, exist_ok=True)
    for relative, data in planned:
        target = out_dir / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)


def count_rows(text: str, extension: str) -> int:
    non_empty = sum(1 for line in text.splitlines() if line.strip())
    if extension.lower() in ("csv", "tsv"):
        return max(0, non_empty - 1)
    return non_empty


def resolve_data_path(manifest_path: Path) -> Path:
    name = manifest_path.name
    stem = name
    for suffix in (".manifest.json", ".json"):
        if name.endswith(suffix):
            stem = name[: -len(suffix)]
            break
    for extension in DATA_EXTENSIONS:
        candidate = manifest_path.parent / f"{stem}.{extension}"
        if candidate.is_file():
            return candidate
    raise DatasetError(
        f"{manifest_path.name}: no data file beside the manifest; expected "
        f"{stem}.(jsonl|txt|csv|tsv)"
    )


def verify_collection(out_dir: Path) -> dict[str, Any]:
    problems: list[str] = []

    def check(condition: bool, message: str) -> None:
        if not condition:
            problems.append(message)

    collection_path = out_dir / COLLECTION_NAME
    if not collection_path.is_file():
        raise DatasetError(f"missing {COLLECTION_NAME} in {out_dir}")
    try:
        collection = json.loads(collection_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise DatasetError(f"{COLLECTION_NAME}: cannot parse ({error})") from error
    if not isinstance(collection, dict):
        raise DatasetError(f"{COLLECTION_NAME}: must be a JSON object")
    check(collection.get("tool") == TOOL, "collection: wrong tool")
    check(collection.get("schema") == SPEC_SCHEMA, "collection: wrong schema")
    check(collection.get("tool_version") == TOOL_VERSION, "collection: wrong tool_version")
    eval_percent = collection.get("eval_percent")
    check(
        isinstance(eval_percent, int) and 1 <= eval_percent <= 50,
        "collection: invalid eval_percent",
    )
    outputs = collection.get("outputs")
    if not isinstance(outputs, dict):
        raise DatasetError("collection: `outputs` is missing")
    declared: set[str] = set()
    split_rows: dict[str, list[dict[str, str]]] = {"train": [], "eval": []}
    for split in SPLITS:
        entries = outputs.get(split)
        check(isinstance(entries, list), f"collection: outputs.{split} must be a list")
        for entry in entries or []:
            if not isinstance(entry, dict):
                problems.append(f"collection: outputs.{split} entry is not an object")
                continue
            manifest_rel = entry.get("manifest")
            data_rel = entry.get("data")
            if not isinstance(manifest_rel, str) or not isinstance(data_rel, str):
                problems.append(f"collection: outputs.{split} entry lacks manifest/data")
                continue
            declared.add(manifest_rel)
            manifest_path = out_dir / manifest_rel
            if not manifest_path.is_file():
                problems.append(f"{manifest_rel}: missing")
                continue
            raw = manifest_path.read_bytes()
            check(
                sha256_bytes(raw) == entry.get("manifest_sha256"),
                f"{manifest_rel}: manifest checksum does not match collection",
            )
            try:
                document = json.loads(raw.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                problems.append(f"{manifest_rel}: cannot parse ({error})")
                continue
            if not isinstance(document, dict):
                problems.append(f"{manifest_rel}: manifest is not an object")
                continue
            check(document.get("schema") == MANIFEST_SCHEMA, f"{manifest_rel}: schema != 1")
            check(
                isinstance(document.get("name"), str) and document["name"].strip(),
                f"{manifest_rel}: blank name",
            )
            check(
                isinstance(document.get("version"), str) and document["version"].strip(),
                f"{manifest_rel}: blank version",
            )
            check(
                isinstance(document.get("source"), str) and document["source"].strip(),
                f"{manifest_rel}: blank source",
            )
            license_id = document.get("license")
            check(license_id in LICENSES, f"{manifest_rel}: unknown license {license_id!r}")
            check(
                license_id == entry.get("license"),
                f"{manifest_rel}: license does not match collection",
            )
            check(
                document.get("source") == entry.get("source"),
                f"{manifest_rel}: source does not match collection",
            )
            check(
                document.get("split") == split,
                f"{manifest_rel}: split {document.get('split')!r} != {split}",
            )
            check(
                document.get("name") == entry.get("name"),
                f"{manifest_rel}: name does not match collection",
            )
            sha = document.get("sha256")
            check(
                isinstance(sha, str)
                and len(sha) == 64
                and all(ch in "0123456789abcdef" for ch in sha),
                f"{manifest_rel}: invalid sha256",
            )
            notes = document.get("notes")
            check(
                isinstance(notes, str) and len(notes) <= MAX_NOTES_CHARS,
                f"{manifest_rel}: missing or too-long notes",
            )
            languages = document.get("languages")
            check(
                isinstance(languages, list) and bool(languages),
                f"{manifest_rel}: languages must be a non-empty list",
            )
            data_path = out_dir / data_rel
            check(data_path.is_file(), f"{data_rel}: missing")
            if not data_path.is_file():
                continue
            try:
                resolved_data = resolve_data_path(manifest_path)
            except DatasetError as error:
                problems.append(str(error))
                resolved_data = None
            if resolved_data is not None:
                check(
                    resolved_data.resolve() == data_path.resolve(),
                    f"{manifest_rel}: data file does not follow the manifest name",
                )
            data = data_path.read_bytes()
            check(
                sha256_bytes(data) == sha,
                f"{data_rel}: sha256 does not match its manifest",
            )
            check(
                sha256_bytes(data) == entry.get("data_sha256"),
                f"{data_rel}: sha256 does not match collection",
            )
            try:
                text = data.decode("utf-8")
            except UnicodeDecodeError:
                problems.append(f"{data_rel}: not valid UTF-8")
                continue
            rows = count_rows(text, data_path.suffix.lstrip("."))
            check(rows == document.get("rows"), f"{data_rel}: row count mismatch")
            check(rows == entry.get("rows"), f"{data_rel}: collection row count mismatch")
            role = entry.get("role")
            check(role in ROLES, f"{manifest_rel}: invalid role {role!r}")
            for line_number, line in enumerate(text.splitlines(), 1):
                if not line.strip():
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError as error:
                    problems.append(f"{data_rel}:{line_number}: malformed row ({error.msg})")
                    continue
                if not isinstance(row, dict) or not isinstance(row.get("case_id"), str):
                    problems.append(f"{data_rel}:{line_number}: missing case_id")
                    continue
                split_rows[split].append(row)
                expected = split_for(
                    row["case_id"], role if role in ROLES else "hash", eval_percent
                )
                if expected != split:
                    problems.append(
                        f"{data_rel}: case_id {row['case_id']!r} routes to "
                        f"{expected}, not {split}"
                    )

    frequency = collection.get("frequency")
    if frequency:
        entries = outputs.get("frequency")
        check(bool(entries), "collection: frequency block present but no frequency output")
        for entry in entries or []:
            manifest_rel = entry.get("manifest")
            data_rel = entry.get("data")
            if not isinstance(manifest_rel, str) or not isinstance(data_rel, str):
                problems.append("collection: frequency output lacks manifest/data")
                continue
            declared.add(manifest_rel)
            manifest_path = out_dir / manifest_rel
            data_path = out_dir / data_rel
            check(manifest_path.is_file(), f"{manifest_rel}: missing")
            check(data_path.is_file(), f"{data_rel}: missing")
            if not (manifest_path.is_file() and data_path.is_file()):
                continue
            manifest_bytes = manifest_path.read_bytes()
            document = json.loads(manifest_bytes.decode("utf-8"))
            data = data_path.read_bytes()
            check(
                document.get("schema") == MANIFEST_SCHEMA,
                f"{manifest_rel}: schema != 1",
            )
            check(document.get("split") == "train", f"{manifest_rel}: split must be train")
            check(document.get("license") in LICENSES, f"{manifest_rel}: unknown license")
            check(
                sha256_bytes(data) == document.get("sha256"),
                f"{data_rel}: sha256 does not match its manifest",
            )
            check(
                sha256_bytes(manifest_bytes) == entry.get("manifest_sha256"),
                f"{manifest_rel}: manifest checksum does not match collection",
            )
            text = data.decode("utf-8")
            rows = count_rows(text, "tsv")
            check(rows == document.get("rows"), f"{data_rel}: row count mismatch")

            counter: Counter[str] = Counter()
            for row in split_rows["train"]:
                counter.update(bangla_tokens(row))
            try:
                expected, stats = frequency_payload(
                    counter,
                    int(frequency.get("min_count", 1)),
                    int(frequency.get("top_tokens", 0)),
                )
            except DatasetError as error:
                problems.append(f"{data_rel}: cannot recompute ({error})")
                continue
            check(
                expected == data,
                f"{data_rel}: does not match a fresh recomputation from train rows",
            )
            check(
                stats["tokens_total"] == collection.get("totals", {}).get("bangla_tokens"),
                "collection: bangla_tokens total mismatch",
            )
            check(
                stats["zipf_buckets"] == frequency.get("zipf_buckets"),
                "collection: zipf bucket mismatch",
            )
    elif outputs.get("frequency"):
        problems.append("collection: frequency output without a frequency block")

    if not frequency:
        train_has_bn = any(any(True for _ in bangla_tokens(row)) for row in split_rows["train"])
        if train_has_bn:
            problems.append(
                "collection: train rows contain Bangla tokens but no frequency output"
            )

    for directory in ("train", "eval", "freq"):
        folder = out_dir / directory
        if not folder.is_dir():
            continue
        for path in sorted(folder.glob("*.manifest.json")):
            relative = path.relative_to(out_dir).as_posix()
            if relative not in declared:
                problems.append(f"{relative}: undeclared manifest in output tree")

    train_ids = {row.get("case_id") for row in split_rows["train"]}
    eval_ids = {row.get("case_id") for row in split_rows["eval"]}
    overlap = sorted(train_ids & eval_ids)
    if overlap:
        problems.append(
            f"contamination: {len(overlap)} case_id values in both train and eval "
            f"(e.g. {', '.join(str(value) for value in overlap[:3])})"
        )

    if problems:
        return {"ok": False, "problems": problems[:50], "problem_count": len(problems)}
    return {
        "ok": True,
        "mode": "verify",
        "name": collection.get("name"),
        "version": collection.get("version"),
        "train_rows": len(split_rows["train"]),
        "eval_rows": len(split_rows["eval"]),
        "bangla_tokens": collection.get("totals", {}).get("bangla_tokens", 0),
        "collection_sha256": sha256_bytes(collection_path.read_bytes()),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Prepare licensed/synthetic corpora into Okkhor dataset manifests."
    )
    parser.add_argument("--input", help="operator corpus directory (build mode)")
    parser.add_argument("--out", required=True, help="output directory")
    parser.add_argument("--spec", default=SPEC_NAME, help="source manifest filename")
    parser.add_argument("--dry-run", action="store_true", help="validate and print, write nothing")
    parser.add_argument("--verify", action="store_true", help="verify an existing output tree")
    parser.add_argument("--force", action="store_true", help="overwrite generated files")
    parser.add_argument("--eval-percent", type=int, help="override the eval bucket threshold")
    parser.add_argument("--min-bn-count", type=int, default=1, help="minimum token count")
    parser.add_argument("--top-tokens", type=int, default=0, help="limit frequency rows (0=all)")
    parser.add_argument(
        "--max-rows", type=int, default=DEFAULT_MAX_ROWS, help="per-file row cap"
    )
    parser.add_argument(
        "--max-file-mb", type=int, default=DEFAULT_MAX_FILE_MB, help="per-file size cap"
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.dry_run and args.verify:
        parser.error("--dry-run and --verify are mutually exclusive")
    if args.verify:
        if not args.out:
            parser.error("--verify requires --out")
        try:
            result = verify_collection(Path(args.out).resolve())
        except DatasetError as error:
            print(json.dumps({"ok": False, "problems": [str(error)]}, sort_keys=True))
            return 1
        print(pretty_json(result))
        return 0 if result["ok"] else 1
    if not args.input:
        parser.error("build mode requires --input")
    if args.min_bn_count < 1:
        parser.error("--min-bn-count must be at least 1")
    if args.top_tokens < 0:
        parser.error("--top-tokens must not be negative")
    if args.max_rows < 1:
        parser.error("--max-rows must be at least 1")
    if args.max_file_mb < 1:
        parser.error("--max-file-mb must be at least 1")

    out_dir = Path(args.out).resolve()
    try:
        summary, planned, _, _ = build_plan(args)
        if args.dry_run:
            summary["would_write"] = [relative for relative, _ in planned]
            print(pretty_json(summary))
            return 0
        write_plan(out_dir, planned, args.force)
    except DatasetError as error:
        print(json.dumps({"ok": False, "problems": str(error).splitlines()}, sort_keys=True))
        return 1
    print(pretty_json(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
