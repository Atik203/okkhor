"""Export a trained next-word model to the `OKNM` v1 container.

The app is Rust and must never load PyTorch. This container is versioned,
self-describing, and checksummed:

    magic "OKNM" | u16 version | u32 header_len | header JSON | pad to 64
    vocabulary blob (UTF-8 words, newline separated, id order)
    tensor blocks, each aligned to 64 bytes

Header fields: schema, language, context, vocab, embed, d_model, layers, heads,
ff, dtype, tensors[{name, shape, offset, bytes, dtype, scale_offset?}],
vocab_offset, vocab_bytes.

`--quantize-output int8` stores the 200k-way output weight as int8 with one
scale per row; the encoder stays f32. `golden.json` captures the PyTorch logits
for a fixed input so the Rust runtime can be tested against exact numbers.

Usage:
    python scripts/export-nextword-model.py \
        --input artifacts/nextword-transformer-v1.pt \
        --out artifacts/onet-bn-v1.oknm --language bn --heads 8 \
        --quantize-output int8
"""

import argparse
import hashlib
import json
import struct
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

MAGIC = b"OKNM"
VERSION = 1
ALIGN = 64
LAYER_TENSORS = [
    "self_attn.in_proj_weight",
    "self_attn.in_proj_bias",
    "self_attn.out_proj.weight",
    "self_attn.out_proj.bias",
    "linear1.weight",
    "linear1.bias",
    "linear2.weight",
    "linear2.bias",
    "norm1.weight",
    "norm1.bias",
    "norm2.weight",
    "norm2.bias",
]


class Model(nn.Module):
    """Same shape as the trainer; used for golden vectors only."""

    def __init__(self, vocab: int, embed: int, d_model: int, layers: int, heads: int, ff: int):
        super().__init__()
        self.emb = nn.Embedding(vocab, embed, padding_idx=0)
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


def align(position: int) -> int:
    return (position + ALIGN - 1) // ALIGN * ALIGN


def quantize_rows(tensor: torch.Tensor) -> tuple[np.ndarray, np.ndarray]:
    values = tensor.detach().float().numpy()
    scale = np.abs(values).max(axis=1) / 127.0
    scale[scale == 0] = 1.0
    quantized = np.clip(np.rint(values / scale[:, None]), -127, 127).astype(np.int8)
    return quantized, scale.astype(np.float32)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--language", choices=["bn", "en"], required=True)
    parser.add_argument("--heads", type=int, default=8)
    parser.add_argument("--quantize-output", choices=["none", "int8"], default="none")
    args = parser.parse_args()

    ckpt = torch.load(Path(args.input).expanduser(), map_location="cpu", weights_only=False)
    state = ckpt["state"]
    vocab_map = ckpt["vocab"]
    if not isinstance(vocab_map, dict):
        raise SystemExit("model does not carry a word->id vocabulary")
    words = [None] * len(vocab_map)
    for word, index in vocab_map.items():
        words[index] = word
    if any(word is None for word in words):
        raise SystemExit("vocabulary is not dense")

    metrics = ckpt.get("metrics") or {}
    vocab = int(state["out.weight"].shape[0])
    embed = int(state["emb.weight"].shape[1])
    d_model = int(state["proj.weight"].shape[0])
    layers = 1 + max(
        int(name.split(".")[2]) for name in state if name.startswith("encoder.layers.")
    )
    ff = int(state["encoder.layers.0.linear1.weight"].shape[0])
    context = int(metrics.get("context", 8))
    print(
        f"vocab {vocab} embed {embed} d_model {d_model} layers {layers} "
        f"heads {args.heads} ff {ff} context {context}",
        flush=True,
    )

    blobs: list[tuple[str, np.ndarray, str, np.ndarray | None]] = []
    for layer in range(layers):
        for suffix in LAYER_TENSORS:
            key = f"encoder.layers.{layer}.{suffix}"
            blobs.append(
                (key.removeprefix("encoder.layers."), state[key].detach().float().numpy(), "f32", None)
            )

    ordered: list[tuple[str, np.ndarray, str, np.ndarray | None]] = [
        ("emb.weight", state["emb.weight"].detach().float().numpy(), "f32", None),
        ("proj.weight", state["proj.weight"].detach().float().numpy(), "f32", None),
        ("proj.bias", state["proj.bias"].detach().float().numpy(), "f32", None),
    ]
    ordered += blobs
    if args.quantize_output == "int8":
        packed, scale = quantize_rows(state["out.weight"])
        ordered.append(("out.weight", packed, "i8", None))
        ordered.append(("out.weight.scale", scale, "f32", None))
    else:
        ordered.append(("out.weight", state["out.weight"].detach().float().numpy(), "f32", None))
    ordered.append(("out.bias", state["out.bias"].detach().float().numpy(), "f32", None))

    vocab_blob = "\n".join(words).encode("utf-8")
    header: dict = {
        "schema": 1,
        "format": MAGIC.decode(),
        "version": VERSION,
        "language": args.language,
        "context": context,
        "vocab": vocab,
        "embed": embed,
        "d_model": d_model,
        "layers": layers,
        "heads": args.heads,
        "ff": ff,
        "dtype": "f32",
        "vocab_offset": 0,
        "vocab_bytes": len(vocab_blob),
        "tensors": [],
    }

    payload = bytearray()
    payload += vocab_blob
    for name, array, dtype, _scale in ordered:
        padding = align(len(payload)) - len(payload)
        payload += b"\0" * padding
        offset = len(payload)
        raw = array.tobytes(order="C")
        payload += raw
        header["tensors"].append(
            {
                "name": name,
                "shape": list(array.shape),
                "dtype": dtype,
                "offset": offset,
                "bytes": len(raw),
            }
        )

    header_bytes = json.dumps(header, separators=(",", ":")).encode("utf-8")
    prefix = struct.pack("<4sHI", MAGIC, VERSION, len(header_bytes)) + header_bytes
    prefix += b"\0" * (align(len(prefix)) - len(prefix))
    blob = prefix + bytes(payload)

    out = Path(args.out).expanduser()
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(blob)
    digest = hashlib.sha256(blob).hexdigest().upper()
    manifest = {
        "name": out.stem,
        "version": 1,
        "format": "OKNM1",
        "language": args.language,
        "bytes": len(blob),
        "sha256": digest,
        "source": Path(args.input).name,
        "quantize_output": args.quantize_output,
    }
    out.with_suffix(".manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {out} {len(blob):,} bytes sha256 {digest}", flush=True)

    # Golden vectors: exact inputs and PyTorch logits for the Rust runtime test.
    model = Model(vocab, embed, d_model, layers, args.heads, ff)
    model.load_state_dict(state)
    model.eval()
    generator = torch.Generator().manual_seed(0x4F4B4B48)
    inputs = torch.randint(3, vocab, (8, context), generator=generator)
    with torch.no_grad():
        logits = model(inputs)
    top = logits.topk(5, dim=1)
    golden = {
        "context": context,
        "inputs": inputs.tolist(),
        "top_ids": top.indices.tolist(),
        "top_logits": [[round(float(v), 6) for v in row] for row in top.values.tolist()],
        "argmax_logits": [round(float(v), 6) for v in logits.max(dim=1).values.tolist()],
    }
    out.with_suffix(".golden.json").write_text(json.dumps(golden, indent=2) + "\n", encoding="utf-8")
    print(f"wrote golden vectors ({len(inputs)} rows)", flush=True)

    # Round-trip check of every tensor.
    reader = json.loads(blob[10 : 10 + len(header_bytes)])
    for entry, (name, array, dtype, _scale) in zip(reader["tensors"], ordered):
        start = len(prefix) + entry["offset"]
        stored = np.frombuffer(
            blob,
            dtype=np.int8 if dtype == "i8" else np.float32,
            count=entry["bytes"] // (1 if dtype == "i8" else 4),
            offset=start,
        ).reshape(entry["shape"])
        assert stored.shape == array.shape, (name, stored.shape, array.shape)
        if dtype == "f32":
            assert np.array_equal(stored, array), name
        else:
            assert np.abs(stored.astype(np.int32) - array.astype(np.int32)).max() <= 1, name
    print("round-trip verified", flush=True)


if __name__ == "__main__":
    main()
