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
import queue
import signal
import sys
import threading
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

PAD, EOS = 0, 2
STOP_REQUESTED = False


def _request_stop(signum: int, frame: object) -> None:
    global STOP_REQUESTED
    STOP_REQUESTED = True


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
    parser.add_argument("--compile", action="store_true")
    parser.add_argument("--prefetch", type=int, default=4)
    parser.add_argument(
        "--sampled-softmax",
        type=int,
        default=0,
        help="train the output layer against K shared negatives instead of the full vocabulary",
    )
    parser.add_argument("--sample-temp", type=float, default=0.75)
    parser.add_argument("--sample-tokens", type=int, default=20_000_000)
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


class Prefetcher:
    """Gather windows and pin them on a producer thread while the GPU runs.

    The memmap gather is CPU work; overlapping it with the forward/backward is
    the difference between a GPU-bound loop and a half-idle one.
    """

    def __init__(
        self, windows: Windows, batch: int, device: torch.device, depth: int
    ) -> None:
        self.windows = windows
        self.batch = batch
        self.device = device
        self.pending: queue.Queue = queue.Queue(maxsize=max(2, depth))
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._produce, daemon=True)
        self.thread.start()

    def _produce(self) -> None:
        while not self.stop.is_set():
            x, y = self.windows.sample(self.batch)
            x_t = torch.from_numpy(x.astype(np.int64))
            y_t = torch.from_numpy(y.astype(np.int64))
            if self.device.type == "cuda":
                x_t = x_t.pin_memory()
                y_t = y_t.pin_memory()
            try:
                self.pending.put((x_t, y_t), timeout=1.0)
            except queue.Full:
                continue

    def next(self) -> tuple[torch.Tensor, torch.Tensor]:
        x_t, y_t = self.pending.get()
        return (
            x_t.to(self.device, non_blocking=True),
            y_t.to(self.device, non_blocking=True),
        )

    def close(self) -> None:
        self.stop.set()


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

            def encode(self, x: torch.Tensor) -> torch.Tensor:
                return self.net[:-1](self.emb(x).flatten(1))

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

            def encode(self, x: torch.Tensor) -> torch.Tensor:
                return self.encoder(self.proj(self.emb(x)))[:, -1]

            def forward(self, x: torch.Tensor) -> torch.Tensor:
                return self.out(self.encode(x))

    return Model()


class NegativeSampler:
    """Shared negatives drawn from the corpus unigram^temperature.

    Training against K negatives turns the 200k-way output layer into a
    (K+1)-way problem. The weights are still the full vocabulary, so evaluation
    and inference keep the exact softmax.
    """

    def __init__(
        self,
        data: np.ndarray,
        vocab_size: int,
        device: torch.device,
        temperature: float,
        sample_tokens: int,
    ) -> None:
        chunk = np.asarray(data[:sample_tokens], dtype=np.int64)
        counts = np.bincount(chunk, minlength=vocab_size).astype(np.float64)
        counts[:3] = 0.0  # <pad>, <unk>, <eos> are never suggestions
        weights = counts**temperature
        weights /= weights.sum()
        self.device = device
        probs = torch.tensor(weights, dtype=torch.float32, device=device)
        self.cdf = torch.cumsum(probs, dim=0)
        self.logprob = torch.log(probs.clamp_min(1e-12))

    def sample(self, count: int) -> torch.Tensor:
        draws = torch.rand(count, device=self.device)
        return torch.searchsorted(self.cdf, draws).clamp_(max=self.cdf.numel() - 1)


def sampled_logits(
    model: nn.Module,
    hidden: torch.Tensor,
    target: torch.Tensor,
    sampler: NegativeSampler,
    negatives: int,
) -> torch.Tensor:
    """Logits for the target plus K shared negatives, debiased by log q."""
    weight = model.out.weight
    neg = sampler.sample(negatives)
    w_neg = torch.nn.functional.embedding(neg, weight)
    w_pos = torch.nn.functional.embedding(target, weight)
    positive = (hidden * w_pos).sum(dim=1, keepdim=True) - sampler.logprob[target].unsqueeze(1)
    negative = hidden @ w_neg.t() - sampler.logprob[neg].unsqueeze(0)
    return torch.cat([positive, negative], dim=1)


def evaluate(
    model: nn.Module,
    windows: Windows,
    device: torch.device,
    batch: int,
    samples: int,
) -> tuple[float, float, float, int]:
    model.eval()
    top1 = top3 = top5 = seen = 0
    remaining = samples
    with torch.no_grad():
        while remaining > 0:
            take = min(batch, remaining)
            x, y = windows.sample(take)
            x_t = torch.from_numpy(x.astype(np.int64)).to(device)
            y_t = torch.from_numpy(y.astype(np.int64)).to(device)
            with torch.autocast("cuda", dtype=torch.bfloat16, enabled=device.type == "cuda"):
                logits = model(x_t)
            top = logits.topk(5, dim=1).indices
            top1 += int((top[:, 0] == y_t).sum())
            top3 += int((top[:, :3] == y_t.unsqueeze(1)).any(dim=1).sum())
            top5 += int((top == y_t.unsqueeze(1)).any(dim=1).sum())
            seen += take
            remaining -= take
    model.train()
    return top1 / max(seen, 1), top3 / max(seen, 1), top5 / max(seen, 1), seen


def probe_batch(
    model: nn.Module,
    optimizer: torch.optim.Optimizer,
    windows: Windows,
    device: torch.device,
    batch: int,
    loss_fn: nn.Module,
    amp_dtype: torch.dtype,
) -> bool:
    """One full step at `batch`; returns False on CUDA OOM.

    The step includes `optimizer.step()` on purpose: Adam allocates its state
    lazily on the first update, and a probe without it underestimates peak
    memory by ~2 GB, which is exactly how batch 3584 slipped through and then
    failed in training.

    The OOM is caught here rather than propagated: a traceback would keep the
    failed tensors alive and make the next, smaller probe fail for the wrong
    reason.
    """
    x_t = None
    y_t = None
    try:
        x, y = windows.sample(batch)
        x_t = torch.from_numpy(x.astype(np.int64)).to(device)
        y_t = torch.from_numpy(y.astype(np.int64)).to(device)
        with torch.autocast("cuda", dtype=amp_dtype, enabled=device.type == "cuda"):
            loss = loss_fn(model(x_t), y_t)
        loss.backward()
        optimizer.step()
        optimizer.zero_grad(set_to_none=True)
        return True
    except torch.cuda.OutOfMemoryError:
        return False
    finally:
        del x_t, y_t
        if device.type == "cuda":
            torch.cuda.empty_cache()


def main() -> None:
    args = parse_args()
    vocab = json.loads(Path(args.vocab).expanduser().read_text(encoding="utf-8"))
    vocab_size = len(vocab)
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")

    train_windows = Windows(str(Path(args.tokens).expanduser()), args.context, args.seed)
    eval_windows = Windows(str(Path(args.eval_tokens).expanduser()), args.context, args.seed + 1)
    print(
        f"device {device} lang {args.lang} arch {args.arch} vocab {vocab_size:,} "
        f"train_tokens {train_windows.size:,} eval_tokens {eval_windows.size:,}",
        flush=True,
    )

    model = build_model(args, vocab_size).to(device)
    optimizer = torch.optim.AdamW(
        model.parameters(), lr=args.lr, fused=device.type == "cuda"
    )
    loss_fn = nn.CrossEntropyLoss()
    # bf16 needs no loss scaling on Hopper; keep the scaler disabled so the
    # checkpoint format stays the same.
    amp_dtype = torch.bfloat16 if device.type == "cuda" else torch.float32
    scaler = torch.amp.GradScaler("cuda", enabled=False)

    sampler = None
    if args.sampled_softmax > 0 and device.type == "cuda":
        sampler = NegativeSampler(
            train_windows.data,
            vocab_size,
            device,
            args.sample_temp,
            args.sample_tokens,
        )
        print(
            f"sampled softmax k={args.sampled_softmax} temp={args.sample_temp}",
            flush=True,
        )
    use_compile = args.compile and sampler is None

    # Probe the requested batch and halve until it fits. The probe runs through
    # the compiled callable, so the measurement includes compile workspace. Any
    # compile failure falls back to eager before training starts.
    forward: nn.Module = model
    compiled = False
    compile_blocked = False
    batch = args.batch
    if device.type == "cuda":
        while True:
            try:
                if use_compile and not compiled and not compile_blocked:
                    forward = torch.compile(model)
                    compiled = True
                    print("torch.compile enabled", flush=True)
                if probe_batch(
                    forward, optimizer, train_windows, device, batch, loss_fn, amp_dtype
                ):
                    break
            except Exception as error:  # pragma: no cover - environment dependent
                if not compiled:
                    raise
                print(f"compile unavailable: {error}; using eager", flush=True)
                forward = model
                compiled = False
                compile_blocked = True
                continue
            torch.cuda.empty_cache()
            if batch <= 1024:
                raise SystemExit("batch 1024 does not fit on this slice")
            print(f"OOM at batch {batch}; halving", flush=True)
            batch //= 2
        print(f"batch {batch} fits", flush=True)
        torch.cuda.empty_cache()

    steps_per_epoch = max(1, train_windows.size // batch)
    if args.max_steps:
        steps_per_epoch = min(steps_per_epoch, args.max_steps)
    print(f"steps/epoch {steps_per_epoch:,} batch {batch}", flush=True)

    prefetcher = Prefetcher(train_windows, batch, device, args.prefetch)

    prefix = Path(args.out_prefix).expanduser()
    prefix.parent.mkdir(parents=True, exist_ok=True)
    checkpoint = prefix.with_suffix(".ckpt.pt")
    pause_file = prefix.with_suffix(".pause")
    signal.signal(signal.SIGTERM, _request_stop)
    signal.signal(signal.SIGINT, _request_stop)

    def save_checkpoint(epoch: int, step: int) -> None:
        temp = checkpoint.with_suffix(".tmp")
        torch.save(
            {
                "state": model.state_dict(),
                "optimizer": optimizer.state_dict(),
                "scaler": scaler.state_dict(),
                "epoch": epoch,
                "step": step,
                "args": vars(args),
            },
            temp,
        )
        temp.replace(checkpoint)

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
        running_total = torch.zeros((), device=device)
        steps = 0
        first = start_step if epoch == start_epoch else 0
        for step in range(first, steps_per_epoch):
            if STOP_REQUESTED:
                save_checkpoint(epoch, step)
                print("stop requested; checkpoint saved", flush=True)
                sys.exit(75)
            if pause_file.exists():
                save_checkpoint(epoch, step)
                print(
                    f"paused at epoch {epoch} step {step}; remove {pause_file.name} to resume",
                    flush=True,
                )
                while pause_file.exists() and not STOP_REQUESTED:
                    time.sleep(10)
                if STOP_REQUESTED:
                    print("stop requested while paused", flush=True)
                    sys.exit(75)
                print("resumed", flush=True)
            x_t, y_t = prefetcher.next()
            optimizer.zero_grad(set_to_none=True)
            try:
                with torch.autocast(
                    "cuda", dtype=amp_dtype, enabled=device.type == "cuda"
                ):
                    if sampler is None:
                        loss = loss_fn(forward(x_t), y_t)
                    else:
                        hidden = model.encode(x_t)
                        logits = sampled_logits(
                            model, hidden, y_t, sampler, args.sampled_softmax
                        )
                        loss = loss_fn(logits, torch.zeros_like(y_t))
            except Exception as error:  # pragma: no cover - environment dependent
                if not compiled:
                    raise
                print(f"compile fallback to eager: {error}", flush=True)
                forward = model
                compiled = False
                with torch.autocast(
                    "cuda", dtype=amp_dtype, enabled=device.type == "cuda"
                ):
                    loss = loss_fn(model(x_t), y_t)
            scaler.scale(loss).backward()
            scaler.step(optimizer)
            scaler.update()
            # Keep the running loss on the GPU and sync once per log window;
            # a per-step `item()` stalls the pipeline every step.
            running_total = running_total + loss.detach()
            steps += 1
            global_step += 1
            if steps % args.log_every == 0:
                used = reserved = 0
                if device.type == "cuda":
                    used = torch.cuda.memory_allocated() // (1024**2)
                    reserved = torch.cuda.memory_reserved() // (1024**2)
                elapsed = time.time() - started
                tokens_per_s = batch * steps / max(elapsed, 1e-9)
                print(
                    f"epoch {epoch} step {step} loss {float(running_total) / steps:.4f} "
                    f"vram {used}MiB reserved {reserved}MiB "
                    f"tok/s {tokens_per_s:,.0f} elapsed {elapsed / 60:.0f}m",
                    flush=True,
                )
            if steps % args.ckpt_every == 0:
                save_checkpoint(epoch, step + 1)
        print(
            f"epoch {epoch} done loss {float(running_total) / max(steps, 1):.4f}",
            flush=True,
        )
        top1, top3, top5, seen = evaluate(
            model, eval_windows, device, batch, args.eval_samples
        )
        print(
            f"epoch {epoch} eval top1 {top1:.4f} top3 {top3:.4f} top5 {top5:.4f} n {seen:,}",
            flush=True,
        )
        save_checkpoint(epoch + 1, 0)
        start_step = 0

    top1, top3, top5, seen = evaluate(
        model, eval_windows, device, batch, args.eval_samples
    )
    metrics = {
        "language": args.lang,
        "arch": args.arch,
        "vocab": vocab_size,
        "train_tokens": int(train_windows.size),
        "eval_tokens": int(eval_windows.size),
        "eval_pairs": seen,
        "top1": round(top1, 4),
        "top3": round(top3, 4),
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
