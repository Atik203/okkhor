"""Build the compact `OKNW` next-word prior blobs from trained bigram JSON.

Input JSON shape (produced by the training pipeline):

    {"name": ..., "model": {"<previous>": [["<next>", count], ...], ...}}

Output `OKNW` v1 layout (little-endian):

    header 32 bytes
      0  magic          "OKNW"
      4  version        u16 (=1)
      6  language       u16 (0 = bn, 1 = en)
      8  word_count     u32
     12  word_off       u32
     16  entry_count    u32
     20  entry_off      u32
     24  follow_count   u32
     28  follow_off     u32
    word table 8 bytes each: off u32, len u16, reserved u16
    entry 12 bytes each, sorted by previous-word bytes:
      prev_id u32, follow_start u32, follow_count u16, reserved u16
    follower array: u32 word ids, most frequent first
    string pool: UTF-8

Usage:
    python scripts/build-nextword.py \
        --bangla artifacts/nextword-bigram-v2.json \
        --english artifacts/nextword-bigram-en-v1.json \
        --out crates/okkhor-engine/assets/nextword
"""

import argparse
import hashlib
import json
import struct
from pathlib import Path

MAGIC = b"OKNW"
VERSION = 1
HEADER_LEN = 32
WORD_RECORD = 8
ENTRY_RECORD = 12


def normalize(followers) -> list[str]:
    kept: list[str] = []
    seen: set[str] = set()
    for word, _count in followers:
        if not word or word in seen:
            continue
        seen.add(word)
        kept.append(word)
    return kept


def build(src: Path, language: int) -> bytes:
    data = json.loads(src.read_text(encoding="utf-8"))
    model = data.get("model") or {}
    if not model:
        raise SystemExit(f"{src}: no 'model' object")

    ids: dict[str, int] = {}

    def word_id(word: str) -> int:
        found = ids.get(word)
        if found is not None:
            return found
        new = len(ids)
        ids[word] = new
        return new

    rows: list[tuple[str, list[str]]] = []
    for previous, followers in model.items():
        word_id(previous)
        kept = normalize(followers)
        if kept:
            for word in kept:
                word_id(word)
            rows.append((previous, kept))

    rows.sort(key=lambda row: row[0].encode("utf-8"))

    words = sorted(ids, key=lambda word: ids[word])
    pool = bytearray()
    word_table = bytearray()
    for word in words:
        raw = word.encode("utf-8")
        word_table += struct.pack("<IHH", len(pool), len(raw), 0)
        pool += raw

    follow = bytearray()
    entries = bytearray()
    follow_count = 0
    for previous, followers in rows:
        start = follow_count
        for word in followers:
            follow += struct.pack("<I", ids[word])
            follow_count += 1
        entries += struct.pack("<IIHH", ids[previous], start, len(followers), 0)

    word_off = HEADER_LEN
    entry_off = word_off + len(word_table)
    follow_off = entry_off + len(entries)
    pool_off = follow_off + len(follow)
    header = struct.pack(
        "<4sHHIIIIII",
        MAGIC,
        VERSION,
        language,
        len(words),
        word_off,
        len(rows),
        entry_off,
        follow_count,
        follow_off,
    )
    blob = header + word_table + entries + follow + pool
    print(
        f"{src.name}: words={len(words)} prevs={len(rows)} followers={follow_count} "
        f"pool={len(pool)} bytes={len(blob)} lang={language}"
    )
    return bytes(blob)


def verify(blob: bytes, src: Path) -> None:
    data = json.loads(src.read_text(encoding="utf-8"))["model"]
    (magic, version, _lang, word_count, word_off, entry_count, entry_off,
     follow_count, follow_off) = struct.unpack_from("<4sHHIIIIII", blob, 0)
    assert magic == MAGIC and version == VERSION
    pool_off = follow_off + follow_count * 4
    pool = blob[pool_off:]

    def word(word_id: int) -> str:
        off, length, _ = struct.unpack_from("<IHH", blob, word_off + word_id * WORD_RECORD)
        return pool[off:off + length].decode("utf-8")

    prevs = [word(struct.unpack_from("<I", blob, entry_off + i * ENTRY_RECORD)[0])
             for i in range(entry_count)]
    assert prevs == sorted(prevs, key=lambda w: w.encode("utf-8")), "entries not sorted"
    assert len(set(prevs)) == len(prevs), "duplicate previous words"

    index = {word: i for i, word in enumerate(prevs)}
    checked = 0
    for previous, followers in data.items():
        if previous not in index:
            continue
        _pid, start, count, _ = struct.unpack_from(
            "<IIHH", blob, entry_off + index[previous] * ENTRY_RECORD
        )
        got = [word(struct.unpack_from("<I", blob, follow_off + (start + i) * 4)[0])
               for i in range(count)]
        want = normalize(followers)
        assert got == want, f"{previous!r}: {got[:3]} != {want[:3]}"
        checked += 1
    print(f"  verified {checked} previous words")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bangla", required=True)
    parser.add_argument("--english", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    for src, language, name in (
        (Path(args.bangla), 0, "bangla-bigram-v1.bin"),
        (Path(args.english), 1, "english-bigram-v1.bin"),
    ):
        blob = build(src, language)
        verify(blob, src)
        target = out / name
        target.write_bytes(blob)
        digest = hashlib.sha256(blob).hexdigest().upper()
        print(f"wrote {target} sha256={digest}")
        meta = {
            "name": name.removesuffix("-v1.bin"),
            "version": 1,
            "format": "OKNW1",
            "bytes": len(blob),
            "sha256": digest,
            "source": src.name,
        }
        (out / f"{name.removesuffix('.bin')}.manifest.json").write_text(
            json.dumps(meta, indent=2) + "\n", encoding="utf-8"
        )


if __name__ == "__main__":
    main()
