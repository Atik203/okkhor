"""Build the tiny `OKNM` fixture used by the Rust runtime tests.

Produces, under `crates/okkhor-engine/assets/nextword/`:

* `test-model.oknm` — same layout as a real export, tiny dimensions;
* `test-model.golden.json` — PyTorch logits computed with the dequantized int8
  output weight, i.e. exactly what the Rust path must reproduce.

Deterministic: fixed seed, fixed inputs.
"""

import json
from pathlib import Path

import torch
import torch.nn as nn

from nextword_container import quantize_rows, write_container

VOCAB, EMBED, D_MODEL, LAYERS, HEADS, FF, CONTEXT = 48, 8, 16, 2, 2, 32, 6
SEED = 0x4F4B4B48
OUT = Path(__file__).resolve().parent.parent / "crates" / "okkhor-engine" / "assets" / "nextword"


class Model(nn.Module):
    def __init__(self) -> None:
        super().__init__()
        self.emb = nn.Embedding(VOCAB, EMBED, padding_idx=0)
        self.proj = nn.Linear(EMBED, D_MODEL)
        layer = nn.TransformerEncoderLayer(
            d_model=D_MODEL,
            nhead=HEADS,
            dim_feedforward=FF,
            dropout=0.0,
            batch_first=True,
            norm_first=True,
        )
        self.encoder = nn.TransformerEncoder(layer, LAYERS)
        self.out = nn.Linear(D_MODEL, VOCAB)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.out(self.encoder(self.proj(self.emb(x)))[:, -1])


def main() -> None:
    torch.manual_seed(SEED)
    model = Model()
    model.eval()

    words = ["<pad>", "<unk>", "<eos>"] + [f"w{i}" for i in range(3, VOCAB)]
    assert len(words) == VOCAB

    # Quantize the projection exactly as the exporter does, then evaluate the
    # model with the dequantized weight so PyTorch and Rust compute the same
    # numbers.
    weight = model.out.weight.detach().float().numpy()
    packed, scale_bytes, shape = quantize_rows(weight)
    import numpy as np

    quantized = np.frombuffer(packed, dtype=np.int8).reshape(shape).astype(np.float32)
    scales = np.frombuffer(scale_bytes, dtype=np.float32)
    dequantized = torch.from_numpy(quantized * scales[:, None])

    model.out.weight.data.copy_(dequantized)

    tensors: list[tuple[str, bytes, str, list[int]]] = []

    def f32(name: str, tensor: torch.Tensor) -> None:
        array = tensor.detach().float().numpy()
        tensors.append((name, array.tobytes(order="C"), "f32", list(array.shape)))

    f32("emb.weight", model.emb.weight)
    f32("proj.weight", model.proj.weight)
    f32("proj.bias", model.proj.bias)
    for index, layer in enumerate(model.encoder.layers):
        f32(f"layers.{index}.self_attn.in_proj_weight", layer.self_attn.in_proj_weight)
        f32(f"layers.{index}.self_attn.in_proj_bias", layer.self_attn.in_proj_bias)
        f32(f"layers.{index}.self_attn.out_proj.weight", layer.self_attn.out_proj.weight)
        f32(f"layers.{index}.self_attn.out_proj.bias", layer.self_attn.out_proj.bias)
        f32(f"layers.{index}.linear1.weight", layer.linear1.weight)
        f32(f"layers.{index}.linear1.bias", layer.linear1.bias)
        f32(f"layers.{index}.linear2.weight", layer.linear2.weight)
        f32(f"layers.{index}.linear2.bias", layer.linear2.bias)
        f32(f"layers.{index}.norm1.weight", layer.norm1.weight)
        f32(f"layers.{index}.norm1.bias", layer.norm1.bias)
        f32(f"layers.{index}.norm2.weight", layer.norm2.weight)
        f32(f"layers.{index}.norm2.bias", layer.norm2.bias)
    tensors.append(("out.weight", packed, "i8", shape))
    tensors.append(("out.weight.scale", scale_bytes, "f32", [VOCAB]))
    f32("out.bias", model.out.bias)

    OUT.mkdir(parents=True, exist_ok=True)
    target = OUT / "test-model.oknm"
    manifest = write_container(
        target,
        language="bn",
        words=words,
        tensors=tensors,
        context=CONTEXT,
        embed=EMBED,
        d_model=D_MODEL,
        layers=LAYERS,
        heads=HEADS,
        ff=FF,
        source="fixture",
    )

    generator = torch.Generator().manual_seed(SEED)
    inputs = torch.randint(3, VOCAB, (3, CONTEXT), generator=generator)
    with torch.no_grad():
        logits = model(inputs)
    top = logits.topk(5, dim=1)
    golden = {
        "context": CONTEXT,
        "inputs": inputs.tolist(),
        "top_ids": top.indices.tolist(),
        "top_logits": [[round(float(v), 6) for v in row] for row in top.values.tolist()],
        "max_logits": [round(float(v), 6) for v in logits.max(dim=1).values.tolist()],
        "sum_exp": [round(float(v), 6) for v in torch.exp(logits).sum(dim=1).tolist()],
    }
    (OUT / "test-model.golden.json").write_text(
        json.dumps(golden, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {target} {manifest['bytes']} bytes sha256 {manifest['sha256']}")
    print("golden:", golden["top_ids"])


if __name__ == "__main__":
    main()
