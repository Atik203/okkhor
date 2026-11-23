#!/usr/bin/env python3
"""Build the Okkhor suggestion index (OKS1).

Sources
-------
- Avro Keyboard 5.6.0 `Database.db3` (owner-provided portable build):
  Bangla word tables plus the `Suffix` table.
- Avro `autodict.dct`: roman autocorrect pairs.
- The curated starter list shipped with the daemon.
- `wordfreq`: Bengali popularity and a small English completion list.

The output is deterministic: no timestamps, sorted sections, and a manifest
with source SHA256s. Do not commit source paths; only basenames are recorded.

Usage
-----
    python scripts/build-suggestion-index.py
    python scripts/build-suggestion-index.py --avro-home ../avro_portable

The generated file is committed at
`crates/okkhor-engine/assets/suggest/suggest-v1.bin`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sqlite3
import struct
import sys
import unicodedata
from importlib.metadata import version as package_version
from pathlib import Path

MAGIC = b"OKS1"
VERSION = 1

SECTION_POOL = 1
SECTION_CURATED = 2
SECTION_BN = 3
SECTION_BN_POP = 4
SECTION_AUTOC = 5
SECTION_SUFFIX = 6
SECTION_EN = 7
SECTION_EN_POP = 8
SECTION_META = 9

MAX_WORD_BYTES = 96
MAX_ENGLISH = 60_000
MAX_BANGLA_WORDFREQ = 50_000
ALLOWED_BN = set(range(0x0980, 0x0A00)) | {0x200C, 0x200D}
ROMAN_RE = re.compile(r"[A-Za-z.']+")
ENGLISH_RE = re.compile(r"[a-z][a-z']*")
SUFFIX_ROMAN_RE = re.compile(r"[A-Za-z]+")


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def normalize_bangla(text: str) -> str | None:
    """NFC-normalize and keep only single Bengali words."""
    text = unicodedata.normalize("NFC", text).strip()
    if not text or len(text.encode("utf-8")) > MAX_WORD_BYTES:
        return None
    if unicodedata.category(text[0]) != "Lo":
        return None
    for ch in text:
        codepoint = ord(ch)
        if codepoint in (0x200C, 0x200D):
            continue
        if codepoint not in ALLOWED_BN:
            return None
        if unicodedata.category(ch) not in ("Lo", "Mn", "Mc", "Nd"):
            return None
    return text


def normalize_english(text: str) -> str | None:
    text = unicodedata.normalize("NFC", text).strip().lower()
    if not (2 <= len(text) <= 32) or not ENGLISH_RE.fullmatch(text):
        return None
    return text


def read_avro_words(db_path: Path) -> list[str]:
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        tables = [
            row[0]
            for row in connection.execute(
                "select name from sqlite_master "
                "where type='table' and name != 'Suffix' order by name"
            )
        ]
        words: list[str] = []
        for table in tables:
            words.extend(row[0] for row in connection.execute(f'select Words from "{table}"'))
        return words
    finally:
        connection.close()


def read_suffixes(db_path: Path) -> list[tuple[str, str]]:
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        rows = connection.execute("select English, Bangla from Suffix").fetchall()
    finally:
        connection.close()
    pairs: dict[str, str] = {}
    for roman, bangla in rows:
        if not isinstance(roman, str) or not isinstance(bangla, str):
            continue
        roman = roman.strip()
        bangla = normalize_bangla(bangla)
        if not roman or bangla is None or not SUFFIX_ROMAN_RE.fullmatch(roman):
            continue
        pairs.setdefault(roman, bangla)
    return sorted(pairs.items(), key=lambda item: (-len(item[0]), item[0]))


def read_autocorrect(path: Path) -> list[tuple[str, str]]:
    pairs: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line or line.startswith("/") or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        if len(parts) != 2:
            continue
        left, right = parts[0], parts[1].strip()
        if left == right:
            continue
        if not ROMAN_RE.fullmatch(left) or not ROMAN_RE.fullmatch(right):
            continue
        pairs.setdefault(left, right)
    return sorted(pairs.items(), key=lambda item: item[0])


def read_starter(path: Path) -> list[tuple[str, str]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    pairs: dict[str, str] = {}
    for row in data.get("entries", []):
        roman = row.get("roman", "").strip()
        output = normalize_bangla(row.get("output", ""))
        if not roman or output is None or not ROMAN_RE.fullmatch(roman):
            continue
        pairs.setdefault(roman, output)
    return sorted(pairs.items(), key=lambda item: item[0])


def popularity(text: str, language: str) -> int:
    from wordfreq import zipf_frequency

    value = zipf_frequency(text, language)
    return max(0, min(800, round(value * 100)))


class Pool:
    """Interned UTF-8 string pool shared by all text sections."""

    def __init__(self) -> None:
        self.data = bytearray()
        self.offsets: dict[str, tuple[int, int]] = {}

    def add(self, text: str) -> tuple[int, int]:
        cached = self.offsets.get(text)
        if cached is not None:
            return cached
        encoded = text.encode("utf-8")
        offset = len(self.data)
        self.data.extend(encoded)
        record = (offset, len(encoded))
        self.offsets[text] = record
        return record


def pack_text_entries(pool: Pool, words: list[tuple[str, int]]) -> tuple[bytes, bytes]:
    """Return (text-sorted entries, popularity indexes) for (word, zipf)."""
    ordered = sorted(words, key=lambda item: item[0].encode("utf-8"))
    entries = bytearray()
    for word, score in ordered:
        offset, length = pool.add(word)
        entries.extend(struct.pack("<IHH", offset, length, score))

    ranked = sorted(ordered, key=lambda item: (-item[1], item[0].encode("utf-8")))
    index_of: dict[str, int] = {}
    for index, (word, _score) in enumerate(ordered):
        index_of[word] = index
    popularity = bytearray()
    for word, _score in ranked:
        popularity.extend(struct.pack("<I", index_of[word]))
    return bytes(entries), bytes(popularity)


def build_section(section_id: int, data: bytes, count: int) -> tuple[int, bytes, int]:
    return section_id, data, count


def write_index(
    *,
    output: Path,
    manifest_path: Path,
    curated: list[tuple[str, str]],
    bangla: list[tuple[str, int]],
    english: list[tuple[str, int]],
    autocorrect: list[tuple[str, str]],
    suffixes: list[tuple[str, str]],
    meta: dict,
) -> None:
    output = Path(output)
    manifest_path = Path(manifest_path)
    pool = Pool()

    curated_entries = bytearray()
    for roman, bangla_output in curated:
        roman_offset, roman_length = pool.add(roman)
        out_offset, out_length = pool.add(bangla_output)
        curated_entries.extend(
            struct.pack("<IHIH", roman_offset, roman_length, out_offset, out_length)
        )

    bn_entries, bn_pop = pack_text_entries(pool, bangla)
    en_entries, en_pop = pack_text_entries(pool, english)

    autoc_entries = bytearray()
    for left, right in autocorrect:
        left_offset, left_length = pool.add(left)
        right_offset, right_length = pool.add(right)
        autoc_entries.extend(
            struct.pack("<IHIH", left_offset, left_length, right_offset, right_length)
        )

    suffix_entries = bytearray()
    for roman, bangla_text in suffixes:
        roman_offset, roman_length = pool.add(roman)
        bangla_offset, bangla_length = pool.add(bangla_text)
        suffix_entries.extend(
            struct.pack("<IHIH", roman_offset, roman_length, bangla_offset, bangla_length)
        )

    meta_bytes = json.dumps(meta, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )

    sections = [
        build_section(SECTION_POOL, bytes(pool.data), len(pool.offsets)),
        build_section(SECTION_CURATED, bytes(curated_entries), len(curated)),
        build_section(SECTION_BN, bn_entries, len(bangla)),
        build_section(SECTION_BN_POP, bn_pop, len(bangla)),
        build_section(SECTION_AUTOC, bytes(autoc_entries), len(autocorrect)),
        build_section(SECTION_SUFFIX, bytes(suffix_entries), len(suffixes)),
        build_section(SECTION_EN, en_entries, len(english)),
        build_section(SECTION_EN_POP, en_pop, len(english)),
        build_section(SECTION_META, meta_bytes, 1),
    ]

    header_size = 12 + 16 * len(sections)
    offset = header_size
    table = bytearray()
    body = bytearray()
    for section_id, data, count in sections:
        padding = (-offset) % 4
        if padding:
            body.extend(b"\0" * padding)
            offset += padding
        table.extend(struct.pack("<IIII", section_id, offset, len(data), count))
        body.extend(data)
        offset += len(data)

    blob = struct.pack("<4sHHI", MAGIC, VERSION, len(sections), 0) + bytes(table) + bytes(body)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(blob)

    digest = hashlib.sha256(blob).hexdigest()
    manifest = {
        **meta,
        "index": output.name,
        "bytes": len(blob),
        "sha256": digest,
    }
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {output} ({len(blob):,} bytes, sha256 {digest})")
    print(f"wrote {manifest_path}")


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8")
    root = repo_root()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--avro-home",
        type=Path,
        default=root.parent / "avro_portable",
        help="directory containing Database.db3 and autodict.dct",
    )
    parser.add_argument(
        "--starter",
        type=Path,
        default=root / "apps/okkhor-daemon/assets/suggestions/bn-starter-v1.json",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=root / "crates/okkhor-engine/assets/suggest/suggest-v1.bin",
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=root / "crates/okkhor-engine/assets/suggest/manifest.json",
    )
    parser.add_argument(
        "--skip-wordfreq",
        action="store_true",
        help="build without wordfreq (popularity 0, no extra words)",
    )
    args = parser.parse_args()

    db_path = args.avro_home / "Database.db3"
    auto_path = args.avro_home / "autodict.dct"
    for required in (db_path, auto_path, args.starter):
        if not required.exists():
            parser.error(f"missing source: {required}")

    avro_words = read_avro_words(db_path)
    suffixes = read_suffixes(db_path)
    autocorrect = read_autocorrect(auto_path)
    curated = read_starter(args.starter)

    bangla_words: set[str] = set()
    for word in avro_words:
        normalized = normalize_bangla(word)
        if normalized:
            bangla_words.add(normalized)

    english_words: set[str] = set()
    wf_version = "skipped"
    if not args.skip_wordfreq:
        from wordfreq import top_n_list

        wf_version = package_version("wordfreq")
        for word in top_n_list("bn", MAX_BANGLA_WORDFREQ):
            normalized = normalize_bangla(word)
            if normalized:
                bangla_words.add(normalized)
        for word in top_n_list("en", MAX_ENGLISH):
            normalized = normalize_english(word)
            if normalized:
                english_words.add(normalized)
    else:
        # Offline builds still get a usable English list from the curated
        # starter's Latin keys (handled as suggestions) but no dictionary;
        # normal releases include wordfreq.
        pass

    bangla = [(word, popularity(word, "bn")) for word in sorted(bangla_words)]
    english = [(word, popularity(word, "en")) for word in sorted(english_words)]

    meta = {
        "schema": 1,
        "generator": "build-suggestion-index.py",
        "wordfreq": wf_version,
        "counts": {
            "avro_raw": len(avro_words),
            "bangla": len(bangla),
            "bangla_with_popularity": sum(1 for _, score in bangla if score > 0),
            "english": len(english),
            "curated": len(curated),
            "autocorrect": len(autocorrect),
            "suffix": len(suffixes),
        },
        "sources": {
            "avro_database": {"name": db_path.name, "sha256": sha256(db_path)},
            "avro_autocorrect": {"name": auto_path.name, "sha256": sha256(auto_path)},
            "starter": {"name": args.starter.name, "sha256": sha256(args.starter)},
        },
        "notes": "Bangla words are single-word NFC; ASCII/Latin entries are rejected.",
    }

    write_index(
        output=args.output,
        manifest_path=args.manifest,
        curated=curated,
        bangla=bangla,
        english=english,
        autocorrect=autocorrect,
        suffixes=suffixes,
        meta=meta,
    )
    print(json.dumps(meta["counts"], ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
