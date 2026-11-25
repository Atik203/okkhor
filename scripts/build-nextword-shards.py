"""Build memory-mappable uint32 token shards for 1B-scale next-word training.

Input is the sentence-per-line text produced by `build_1b_*.py` on the training
host. Output is a small, self-describing directory:

    train.bin    uint32 little-endian token ids, sentences separated by <eos>
    eval.bin     same layout, held out by a stable hash of the line
    vocab.json   ["<pad>", "<unk>", "<eos>", word, ...] in id order
    meta.json    counts, sizes, SHA-256 of each bin

`<pad>` is 0, `<unk>` is 1, `<eos>` is 2. The trainer masks `<eos>` inside a
context window to `<pad>`, so a window never leaks across sentences.

Usage:
    python scripts/build-nextword-shards.py \
        --input ~/okkhor-training/corpus/bangla-1b/bangla-1b.txt \
        --out ~/okkhor-training/datasets/bangla-1b-shards \
        --lang bn --vocab-size 200000 --min-count 10
"""

import argparse
import hashlib
import json
import re
import time
from collections import Counter
from pathlib import Path

import numpy as np

PAD, UNK, EOS = 0, 1, 2
SPECIALS = ["<pad>", "<unk>", "<eos>"]

PATTERNS = {
    "bn": re.compile(r"[\u0980-\u09EF\u09F0\u09F1]+"),
    "en": re.compile(r"[A-Za-z']+"),
}


def bucket(line: str, eval_percent: int) -> bool:
    """True when the line belongs to the held-out set (stable across runs)."""
    if eval_percent <= 0:
        return False
    digest = hashlib.blake2b(line.encode("utf-8"), digest_size=8).digest()
    return int.from_bytes(digest, "little") % 100 < eval_percent


def count_words(path: Path, pattern: re.Pattern, eval_percent: int, limit_lines: int) -> Counter:
    counts: Counter = Counter()
    lines = 0
    started = time.time()
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            lines += 1
            if limit_lines and lines > limit_lines:
                break
            if bucket(line, eval_percent):
                continue
            counts.update(pattern.findall(line))
            if lines % 2_000_000 == 0:
                print(
                    f"  counted {lines:,} lines, {len(counts):,} unique, "
                    f"{time.time() - started:.0f}s",
                    flush=True,
                )
    return counts


def write_split(
    path: Path,
    target: Path,
    pattern: re.Pattern,
    vocab: dict[str, int],
    eval_percent: int,
    held_out: bool,
) -> tuple[int, int]:
    """Write one split. Returns (sentences, tokens)."""
    sentences = 0
    tokens = 0
    pending: list[np.ndarray] = []
    pending_tokens = 0
    with target.open("wb") as out:

        def flush() -> None:
            nonlocal pending, pending_tokens
            if pending:
                np.concatenate(pending).astype("<u4").tofile(out)
                pending = []
                pending_tokens = 0

        with path.open(encoding="utf-8") as handle:
            for line in handle:
                if bucket(line, eval_percent) != held_out:
                    continue
                words = pattern.findall(line)
                if not words:
                    continue
                ids = np.fromiter(
                    (vocab.get(word, UNK) for word in words), dtype=np.uint32, count=len(words)
                )
                pending.append(ids)
                pending.append(np.array([EOS], dtype=np.uint32))
                pending_tokens += len(ids) + 1
                sentences += 1
                tokens += len(ids)
                if pending_tokens >= 8_000_000:
                    flush()
                if sentences % 2_000_000 == 0:
                    print(f"  wrote {sentences:,} sentences", flush=True)
        flush()
    return sentences, tokens


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--lang", choices=["bn", "en"], required=True)
    parser.add_argument("--vocab-size", type=int, default=200_000)
    parser.add_argument("--min-count", type=int, default=10)
    parser.add_argument("--eval-percent", type=int, default=2)
    parser.add_argument("--limit-lines", type=int, default=0)
    parser.add_argument("--pattern", default="")
    args = parser.parse_args()

    source = Path(args.input).expanduser()
    out = Path(args.out).expanduser()
    out.mkdir(parents=True, exist_ok=True)
    pattern = re.compile(args.pattern) if args.pattern else PATTERNS[args.lang]

    print(f"counting {source} ({source.stat().st_size / 1e9:.1f} GB)", flush=True)
    counts = count_words(source, pattern, args.eval_percent, args.limit_lines)
    kept = [word for word, count in counts.most_common() if count >= args.min_count]
    kept = kept[: args.vocab_size]
    tokens = SPECIALS + kept
    vocab = {word: index for index, word in enumerate(tokens)}
    print(
        f"vocabulary {len(vocab):,} (from {len(counts):,} unique, min_count {args.min_count})",
        flush=True,
    )
    del counts

    (out / "vocab.json").write_text(
        json.dumps(tokens, ensure_ascii=False), encoding="utf-8"
    )

    train_sentences, train_tokens = write_split(
        source, out / "train.bin", pattern, vocab, args.eval_percent, held_out=False
    )
    print(f"train: {train_sentences:,} sentences, {train_tokens:,} tokens", flush=True)
    eval_sentences, eval_tokens = write_split(
        source, out / "eval.bin", pattern, vocab, args.eval_percent, held_out=True
    )
    print(f"eval: {eval_sentences:,} sentences, {eval_tokens:,} tokens", flush=True)

    meta = {
        "schema": 1,
        "language": args.lang,
        "source": str(source),
        "source_bytes": source.stat().st_size,
        "vocab_size": len(vocab),
        "min_count": args.min_count,
        "eval_percent": args.eval_percent,
        "train_sentences": train_sentences,
        "train_tokens": train_tokens,
        "eval_sentences": eval_sentences,
        "eval_tokens": eval_tokens,
        "train_sha256": sha256(out / "train.bin"),
        "eval_sha256": sha256(out / "eval.bin"),
    }
    (out / "meta.json").write_text(json.dumps(meta, indent=2), encoding="utf-8")
    print(json.dumps(meta, indent=2), flush=True)


if __name__ == "__main__":
    main()
