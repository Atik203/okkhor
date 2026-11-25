"""Offline evaluation for the next-word integration: accuracy, threshold curve
and the local-prior rerank blend.

Runs on CPU so it never competes with training. Reports:

* top-1 / top-3 / top-5 on the chosen eval lines;
* precision and coverage as the confidence threshold rises;
* top-k after blending `log P_model` with a local bigram prior, which is the
  offline proxy for Smriti.

Usage:
    python scripts/eval-nextword-model.py \
        --input artifacts/nextword-transformer-v1.pt \
        --eval-jsonl datasets/phase5-licensed-full/train/wikimedia-bn.jsonl \
        --tail-lines 104681 --samples 20000 \
        --bigram artifacts/nextword-bigram-v2.json
"""

import argparse
import json
import math
import re
from pathlib import Path

import torch
import torch.nn as nn

PATTERNS = {
    "bn": re.compile(r"[\u0980-\u09EF\u09F0\u09F1]+"),
    "en": re.compile(r"[A-Za-z']+"),
}
PAD, UNK, EOS = 0, 1, 2


class Model(nn.Module):
    def __init__(self, vocab: int, embed: int, d_model: int, layers: int, heads: int, ff: int):
        super().__init__()
        self.emb = nn.Embedding(vocab, embed, padding_idx=PAD)
        self.proj = nn.Linear(embed, d_model)
        layer = nn.TransformerEncoderLayer(
            d_model=d_model,
            nhead=heads,
            dim_feedforward=ff,
            dropout=0.0,
            batch_first=True,
            norm_first=True,
        )
        self.encoder = nn.TransformerEncoder(layer, layers)
        self.out = nn.Linear(d_model, vocab)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.out(self.encoder(self.proj(self.emb(x)))[:, -1])


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--eval-jsonl", required=True)
    parser.add_argument("--tail-lines", type=int, default=100_000)
    parser.add_argument("--samples", type=int, default=20_000)
    parser.add_argument("--batch", type=int, default=256)
    parser.add_argument("--heads", type=int, default=8)
    parser.add_argument("--bigram", default="")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    ckpt = torch.load(Path(args.input).expanduser(), map_location="cpu", weights_only=False)
    state = ckpt["state"]
    vocab_map: dict[str, int] = ckpt["vocab"]
    metrics = ckpt.get("metrics") or {}
    language = metrics.get("language", "bn")
    context = int(metrics.get("context", 8))
    vocab = int(state["out.weight"].shape[0])
    embed = int(state["emb.weight"].shape[1])
    d_model = int(state["proj.weight"].shape[0])
    layers = 1 + max(int(n.split(".")[2]) for n in state if n.startswith("encoder.layers."))
    ff = int(state["encoder.layers.0.linear1.weight"].shape[0])

    model = Model(vocab, embed, d_model, layers, args.heads, ff)
    model.load_state_dict(state)
    model.eval()

    pattern = PATTERNS[language]
    lines: list[str] = Path(args.eval_jsonl).expanduser().read_text(encoding="utf-8").splitlines()
    tail = lines[-args.tail_lines :]
    print(f"{language}: {len(tail):,} eval lines, context {context}", flush=True)

    windows: list[tuple[list[int], int, str]] = []
    for line in tail:
        if not line.strip():
            continue
        row = json.loads(line)
        words = pattern.findall(row.get("text") or "")
        if len(words) < 2:
            continue
        ids = [vocab_map.get(word, UNK) for word in words]
        for index in range(1, len(ids)):
            if ids[index] == EOS:
                continue
            window = [PAD] * context + ids[max(0, index - context) : index]
            windows.append((window[-context:], ids[index], words[index - 1]))
    print(f"windows {len(windows):,}", flush=True)
    if not windows:
        raise SystemExit("no evaluation windows")

    generator = torch.Generator().manual_seed(0x4F4B4B48)
    order = torch.randperm(len(windows), generator=generator)[: args.samples].tolist()
    sample = [windows[i] for i in order]

    bigram: dict[str, dict[str, float]] = {}
    if args.bigram:
        raw = json.loads(Path(args.bigram).expanduser().read_text(encoding="utf-8"))["model"]
        bigram = {
            previous: {word: float(count) for word, count in followers}
            for previous, followers in raw.items()
        }
        print(f"bigram previous words {len(bigram):,}", flush=True)

    ids_list = [row[0] for row in sample]
    targets = torch.tensor([row[1] for row in sample])
    prevs = [row[2] for row in sample]

    top1 = top3 = top5 = 0
    probs_top = []
    shown_counts = {}
    thresholds = [0.02, 0.05, 0.08, 0.10, 0.12, 0.15, 0.20, 0.25, 0.30, 0.40]
    for start in range(0, len(sample), args.batch):
        batch_ids = torch.tensor(ids_list[start : start + args.batch], dtype=torch.long)
        with torch.no_grad():
            logits = model(batch_ids)
            probs = torch.softmax(logits, dim=1)
        top = logits.topk(5, dim=1)
        for row in range(batch_ids.shape[0]):
            target = int(targets[start + row])
            guess = top.indices[row].tolist()
            confidence = float(probs[row, guess[0]])
            probs_top.append((guess, [float(probs[row, g]) for g in guess], confidence, target, prevs[start + row]))
            top1 += int(guess[0] == target)
            top3 += int(target in guess[:3])
            top5 += int(target in guess)
            for threshold in thresholds:
                key = threshold
                shown, hit = shown_counts.get(key, (0, 0))
                if confidence >= threshold:
                    shown += 1
                    hit += int(guess[0] == target)
                shown_counts[key] = (shown, hit)

    total = len(sample)
    print(
        f"model: top1 {top1 / total:.4f} top3 {top3 / total:.4f} top5 {top5 / total:.4f} "
        f"(n={total:,})",
        flush=True,
    )
    print("threshold  coverage  precision", flush=True)
    curve = []
    for threshold in thresholds:
        shown, hit = shown_counts.get(threshold, (0, 0))
        coverage = shown / total
        precision = hit / shown if shown else 0.0
        curve.append({"threshold": threshold, "coverage": round(coverage, 4), "precision": round(precision, 4)})
        print(f"  {threshold:5.2f}    {coverage:7.1%}   {precision:7.1%}", flush=True)

    id_to_word = [None] * vocab
    for word, index in vocab_map.items():
        if 0 <= index < vocab:
            id_to_word[index] = word

    rerank_rows = []
    if bigram:
        for alpha in (0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.7, 1.0, 1.5):
            r1 = r3 = r5 = 0
            for guess, guess_probs, _confidence, _target, previous in probs_top:
                followers = bigram.get(previous)
                if followers:
                    local_total = sum(followers.values())
                    scored = []
                    for word_id, probability in zip(guess, guess_probs):
                        word = id_to_word[word_id] or ""
                        count = followers.get(word, 0.0)
                        local = (count + 0.5) / (local_total + 0.5 * len(followers))
                        scored.append(math.log(max(probability, 1e-12)) + alpha * math.log(local))
                    order = sorted(range(len(scored)), key=lambda i: scored[i], reverse=True)
                    guess = [guess[i] for i in order]
                r1 += int(guess[0] == _target)
                r3 += int(_target in guess[:3])
                r5 += int(_target in guess)
            rerank_rows.append(
                {
                    "alpha": alpha,
                    "top1": round(r1 / total, 4),
                    "top3": round(r3 / total, 4),
                    "top5": round(r5 / total, 4),
                }
            )
        print("alpha   top1      top3      top5", flush=True)
        for row in rerank_rows:
            print(
                f"  {row['alpha']:4.1f}  {row['top1']:.4f}   {row['top3']:.4f}   {row['top5']:.4f}",
                flush=True,
            )

    if args.out:
        Path(args.out).write_text(
            json.dumps(
                {
                    "top1": top1 / total,
                    "top3": top3 / total,
                    "top5": top5 / total,
                    "thresholds": curve,
                    "rerank": rerank_rows,
                },
                indent=2,
            ),
            encoding="utf-8",
        )


if __name__ == "__main__":
    main()
