"""Export a trained next-word model to the `OKNM` v1 container.

The app is Rust and must never load PyTorch. See `nextword_container.py` for the
byte layout and `docs/superpowers/plans/2026-11-25-nextword-integration.md` for
the contract.

Usage:
    python scripts/export-nextword-model.py \
        --input artifacts/nextword-transformer-v1.pt \
        --out artifacts/onet-bn-v1.oknm --language bn --heads 8 \
        --quantize-output int8
"""

import argparse
import json
from pathlib import Path

import numpy as np

import torch
import torch.nn as nn

from nextword_container import quantize_rows, write_container

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


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--language", choices=["bn", "en"], required=True)
    parser.add_argument("--heads", type=int, default=8)
    parser.add_argument("--quantize-output", choices=["none", "int8"], default="none")
    parser.add_argument("--quantize-encoder", choices=["none", "int8"], default="none")
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
    layers = 1 + max(int(n.split(".")[2]) for n in state if n.startswith("encoder.layers."))
    ff = int(state["encoder.layers.0.linear1.weight"].shape[0])
    context = int(metrics.get("context", 8))
    print(
        f"vocab {vocab} embed {embed} d_model {d_model} layers {layers} "
        f"heads {args.heads} ff {ff} context {context}",
        flush=True,
    )

    def f32(name: str) -> tuple[str, bytes, str, list[int]]:
        array = state[name].detach().float().numpy()
        return name, array.tobytes(order="C"), "f32", list(array.shape)

    # Golden vectors must use the *dequantized* weights, otherwise PyTorch and
    # Rust would legitimately disagree.
    effective = {key: value.detach().float().clone() for key, value in state.items()}
    encoder_matrices = {
        "proj.weight",
        "self_attn.in_proj_weight",
        "self_attn.out_proj.weight",
        "linear1.weight",
        "linear2.weight",
    }

    def quantized_pair(
        source: str, name: str
    ) -> list[tuple[str, bytes, str, list[int]]]:
        packed, scale, shape = quantize_rows(state[source].detach().float().numpy())
        dequant = np.frombuffer(packed, dtype=np.int8).reshape(shape).astype(np.float32)
        dequant = dequant * np.frombuffer(scale, dtype=np.float32)[:, None]
        effective[source] = torch.from_numpy(dequant.copy())
        return [
            (name, packed, "i8", shape),
            (f"{name}.scale", scale, "f32", [shape[0]]),
        ]

    tensors: list[tuple[str, bytes, str, list[int]]] = [f32("emb.weight"), f32("proj.bias")]
    if args.quantize_encoder == "int8":
        tensors += quantized_pair("proj.weight", "proj.weight")
    else:
        tensors.append(f32("proj.weight"))
    for layer in range(layers):
        for suffix in LAYER_TENSORS:
            key = f"encoder.layers.{layer}.{suffix}"
            name = f"layers.{layer}.{suffix}"
            if args.quantize_encoder == "int8" and suffix in encoder_matrices:
                tensors += quantized_pair(key, name)
            else:
                array = state[key].detach().float().numpy()
                tensors.append((name, array.tobytes(order="C"), "f32", list(array.shape)))

    if args.quantize_output == "int8":
        tensors += quantized_pair("out.weight", "out.weight")
    else:
        tensors.append(f32("out.weight"))
    tensors.append(f32("out.bias"))
    out = Path(args.out).expanduser()
    out.parent.mkdir(parents=True, exist_ok=True)
    manifest = write_container(
        out,
        language=args.language,
        words=words,
        tensors=tensors,
        context=context,
        embed=embed,
        d_model=d_model,
        layers=layers,
        heads=args.heads,
        ff=ff,
        source=Path(args.input).name,
    )
    print(f"wrote {out} {manifest['bytes']:,} bytes sha256 {manifest['sha256']}", flush=True)

    model = Model(vocab, embed, d_model, layers, args.heads, ff)
    model.load_state_dict(effective)
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
        "max_logits": [round(float(v), 6) for v in logits.max(dim=1).values.tolist()],
    }
    out.with_suffix(".golden.json").write_text(json.dumps(golden, indent=2) + "\n", encoding="utf-8")
    print("wrote golden vectors", flush=True)

    # Round-trip: every tensor reads back byte-identically (f32) or within 1 LSB.
    blob = out.read_bytes()
    header_len = int.from_bytes(blob[6:10], "little")
    header = json.loads(blob[10 : 10 + header_len])
    prefix_len = 10 + header_len
    prefix_len += (-prefix_len) % 64
    by_name = {entry["name"]: entry for entry in header["tensors"]}
    for name, raw, dtype, shape in tensors:
        entry = by_name[name]
        start = prefix_len + entry["offset"]
        width = 1 if dtype == "i8" else 4
        stored = np.frombuffer(blob, dtype=np.int8 if dtype == "i8" else np.float32,
                               count=entry["bytes"] // width, offset=start)
        assert stored.size == int(np.prod(shape)), (name, stored.size, shape)
        if dtype == "f32":
            assert np.array_equal(stored, np.frombuffer(raw, dtype=np.float32)), name
    print("round-trip verified", flush=True)


if __name__ == "__main__":
    main()
