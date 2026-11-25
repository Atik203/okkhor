"""Memory-mapped next-word trainer for 1B-scale token shards.

Reads the `train.bin` / `eval.bin` / `vocab.json` produced by
`scripts/build-nextword-shards.py`. Batches are random windows gathered straight
from the memmap, so RAM stays flat regardless of corpus size.

`<eos>` inside a context window is masked to `<pad>`, so a window never crosses
a sentence boundary. Checkpoints are written periodically and the run resumes
from the latest one.

Usage (on the training host):
    python scripts/train-nextword-stream.py \
        --tokens ~/okkhor-training/datasets/bangla-1b-shards/train.bin \
        --eval-tokens ~/okkhor-training/datasets/bangla-1b-shards/eval.bin \
        --vocab ~/okkhor-training/datasets/bangla-1b-shards/vocab.json \
        --out-prefix ~/okkhor-training/artifacts/nextword-1b-bn \
        --arch transformer --context 8 --d-model 768 --layers 6 --heads 12 \
        --ff 3072 --batch 4096 --epochs 1 --resume
"""

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

PAD, EOS = 0, 2


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tokens", required=True)
    parser.add_argument("--eval-tokens", required=True)
    parser.add_argument("--vocab", required=True)
    parser.add_argument("--out-prefix", required=True)
    parser.add_argument("--lang", default="bn")
    parser.add_argument("--arch", choices=["transformer", "mlp"], default="transformer")
    parser.add_argument("--context", type=int, default=8)
    parser.add_argument("--embed", type=int, default=256)
    parser.add_argument("--d-model", type=int, default=768)
    parser.add_argument("--layers", type=int, default=6)
    parser.add_argument("--heads", type=int, default=12)
    parser.add_argument("--ff", type=int, default=3072)
    parser.add_argument("--hidden", type=int, default=512)
    parser.add_argument("--batch", type=int, default=4096)
    parser.add_argument("--epochs", type=int, default=1)
    parser.add_argument("--lr", type=float, default=3e-4)
    parser.add_argument("--eval-samples", type=int, default=200_000)
    parser.add_argument("--ckpt-every", type=int, default=2_000)
    parser.add_argument("--log-every", type=int, default=250)
    parser.add_argument("--max-steps", type=int, default=0)
    parser.add_argument("--seed", type=int, default=0x4F4B4B48)
    parser.add_argument("--resume", action="store_true")
    return parser.parse_args()


class Windows:
    """Random context windows over a uint32 memmap."""

    def __init__(self, path: str, context: int, seed: int):
        self.data = np.memmap(path, dtype="<u4", mode="r")
        self.context = context
        self.rng = np.random.default_rng(seed)
        self.size = len(self.data)

    def sample(self, batch: int) -> tuple[np.ndarray, np.ndarray]:
        if self.size <= self.context + 1:
            raise SystemExit("token shard is too small for the requested context")
        wanted = batch
        picks: list[np.ndarray] = []
        while sum(len(chunk) for chunk in picks) < wanted:
            draw = self.rng.integers(self.context, self.size, size=batch * 2)
            target = self.data[draw]
            draw = draw[(target != EOS) & (target != PAD)]
            picks.append(draw)
        index = np.concatenate(picks)[:wanted]
        offsets = np.arange(-self.context, 1)
        window = self.data[index[:, None] + offsets]
        window = np.where(window == EOS, PAD, window)
        return window[:, :-1], window[:, -1]


def build_model(args: argparse.Namespace, vocab_size: int) -> nn.Module:
    context = args.context
    if args.arch == "mlp":

        class Model(nn.Module):
            def __init__(self) -> None:
                super().__init__()
                self.emb = nn.Embedding(vocab_size, args.embed, padding_idx=PAD)
                self.net = nn.Sequential(
                    nn.Linear(args.embed * context, args.hidden),
                    nn.ReLU(),
                    nn.Linear(args.hidden, vocab_size),
                )

            def forward(self, x: torch.Tensor) -> torch.Tensor:
                return self.net(self.emb(x).flatten(1))

    else:

        class Model(nn.Module):
            def __init__(self) -> None:
                super().__init__()
                self.emb = nn.Embedding(vocab_size, args.embed, padding_idx=PAD)
                self.proj = nn.Linear(args.embed, args.d_model)
                layer = nn.TransformerEncoderLayer(
                    d_model=args.d_model,
                    nhead=args.heads,
                    dim_feedforward=args.ff,
                    dropout=0.1,
                    batch_first=True,
                    norm_first=True,
                )
                self.encoder = nn.TransformerEncoder(layer, args.layers)
                self.out = nn.Linear(args.d_model, vocab_size)

            def forward(self, x: torch.Tensor) -> torch.Tensor:
                hidden = self.encoder(self.proj(self.emb(x)))
                return self.out(hidden[:, -1])

    return Model()


def evaluate(
    model: nn.Module,
    windows: Windows,
    device: torch.device,
    batch: int,
    samples: int,
) -> tuple[float, float, int]:
    model.eval()
    top1 = top5 = seen = 0
    remaining = samples
    with torch.no_grad():
        while remaining > 0:
            take = min(batch, remaining)
            x, y = windows.sample(take)
            x_t = torch.from_numpy(x.astype(np.int64)).to(device)
            y_t = torch.from_numpy(y.astype(np.int64)).to(device)
            with torch.autocast("cuda", dtype=torch.float16, enabled=device.type == "cuda"):
                logits = model(x_t)
            top = logits.topk(5, dim=1).indices
            top1 += int((top[:, 0] == y_t).sum())
            top5 += int((top == y_t.unsqueeze(1)).any(dim=1).sum())
            seen += take
            remaining -= take
    model.train()
    return top1 / max(seen, 1), top5 / max(seen, 1), seen


def main() -> None:
    args = parse_args()
    vocab = json.loads(Path(args.vocab).expanduser().read_text(encoding="utf-8"))
    vocab_size = len(vocab)
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")

    train_windows = Windows(str(Path(args.tokens).expanduser()), args.context, args.seed)
    eval_windows = Windows(str(Path(args.eval_tokens).expanduser()), args.context, args.seed + 1)
    steps_per_epoch = max(1, train_windows.size // args.batch)
    if args.max_steps:
        steps_per_epoch = min(steps_per_epoch, args.max_steps)
    print(
        f"device {device} lang {args.lang} arch {args.arch} vocab {vocab_size:,} "
        f"train_tokens {train_windows.size:,} eval_tokens {eval_windows.size:,} "
        f"steps/epoch {steps_per_epoch:,}",
        flush=True,
    )

    model = build_model(args, vocab_size).to(device)
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.lr)
    loss_fn = nn.CrossEntropyLoss()
    scaler = torch.amp.GradScaler("cuda", enabled=device.type == "cuda")

    prefix = Path(args.out_prefix).expanduser()
    prefix.parent.mkdir(parents=True, exist_ok=True)
    checkpoint = prefix.with_suffix(".ckpt.pt")
    start_epoch = 0
    start_step = 0
    if args.resume and checkpoint.exists():
        saved = torch.load(checkpoint, map_location=device, weights_only=False)
        model.load_state_dict(saved["state"])
        optimizer.load_state_dict(saved["optimizer"])
        scaler.load_state_dict(saved["scaler"])
        start_epoch = saved["epoch"]
        start_step = saved["step"]
        print(f"resumed epoch {start_epoch} step {start_step}", flush=True)

    started = time.time()
    global_step = start_step
    for epoch in range(start_epoch, args.epochs):
        model.train()
        running = 0.0
        steps = 0
        first = start_step if epoch == start_epoch else 0
        for step in range(first, steps_per_epoch):
            x, y = train_windows.sample(args.batch)
            x_t = torch.from_numpy(x.astype(np.int64)).to(device)
            y_t = torch.from_numpy(y.astype(np.int64)).to(device)
            optimizer.zero_grad(set_to_none=True)
            with torch.autocast("cuda", dtype=torch.float16, enabled=device.type == "cuda"):
                loss = loss_fn(model(x_t), y_t)
            scaler.scale(loss).backward()
            scaler.step(optimizer)
            scaler.update()
            running += float(loss.item())
            steps += 1
            global_step += 1
            if steps % args.log_every == 0:
                used = (
                    torch.cuda.memory_allocated() // (1024**2)
                    if device.type == "cuda"
                    else 0
                )
                elapsed = time.time() - started
                print(
                    f"epoch {epoch} step {step} loss {running / steps:.4f} "
                    f"vram {used}MiB elapsed {elapsed / 60:.0f}m",
                    flush=True,
                )
            if steps % args.ckpt_every == 0:
                torch.save(
                    {
                        "state": model.state_dict(),
                        "optimizer": optimizer.state_dict(),
                        "scaler": scaler.state_dict(),
                        "epoch": epoch,
                        "step": step + 1,
                        "args": vars(args),
                    },
                    checkpoint,
                )
        print(f"epoch {epoch} done loss {running / max(steps, 1):.4f}", flush=True)
        top1, top5, seen = evaluate(
            model, eval_windows, device, args.batch, args.eval_samples
        )
        print(f"epoch {epoch} eval top1 {top1:.4f} top5 {top5:.4f} n {seen:,}", flush=True)
        torch.save(
            {
                "state": model.state_dict(),
                "optimizer": optimizer.state_dict(),
                "scaler": scaler.state_dict(),
                "epoch": epoch + 1,
                "step": 0,
                "args": vars(args),
            },
            checkpoint,
        )
        start_step = 0

    top1, top5, seen = evaluate(model, eval_windows, device, args.batch, args.eval_samples)
    metrics = {
        "language": args.lang,
        "arch": args.arch,
        "vocab": vocab_size,
        "train_tokens": int(train_windows.size),
        "eval_tokens": int(eval_windows.size),
        "eval_pairs": seen,
        "top1": round(top1, 4),
        "top5": round(top5, 4),
        "context": args.context,
        "d_model": args.d_model,
        "layers": args.layers,
        "epochs": args.epochs,
        "steps": global_step,
        "minutes": round((time.time() - started) / 60, 1),
    }
    prefix.with_suffix(".metrics.json").write_text(
        json.dumps(metrics, indent=2), encoding="utf-8"
    )
    torch.save(
        {"state": model.state_dict(), "vocab": vocab_size, "metrics": metrics},
        prefix.with_suffix(".pt"),
    )
    print(json.dumps(metrics, indent=2), flush=True)


if __name__ == "__main__":
    main()
