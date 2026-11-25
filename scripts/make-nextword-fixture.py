"""Build the tiny `OKNM` fixture used by the Rust runtime tests.

Produces, under `crates/okkhor-engine/assets/nextword/`:

* `test-model.oknm` — same layout as a real export, tiny dimensions, with the
  encoder matrices and the projection stored as int8;
* `test-model.golden.json` — PyTorch logits computed with the **dequantized**
  weights, i.e. exactly what the Rust path must reproduce.

Deterministic: fixed seed, fixed inputs.
"""

import json
from pathlib import Path

import numpy as np
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


def quantize_into(tensor: torch.Tensor) -> tuple[bytes, bytes, list[int]]:
    """Quantize in place to the dequantized value, returning the stored bytes."""
    packed, scale_bytes, shape = quantize_rows(tensor.detach().float().numpy())
    quantized = np.frombuffer(packed, dtype=np.int8).reshape(shape).astype(np.float32)
    dequantized = quantized * np.frombuffer(scale_bytes, dtype=np.float32)[:, None]
    tensor.data.copy_(torch.from_numpy(dequantized.copy()))
    return packed, scale_bytes, shape


def main() -> None:
    torch.manual_seed(SEED)
    model = Model()
    model.eval()

    words = ["<pad>", "<unk>", "<eos>"] + [f"w{i}" for i in range(3, VOCAB)]
    assert len(words) == VOCAB

    tensors: list[tuple[str, bytes, str, list[int]]] = []

    def f32(name: str, tensor: torch.Tensor) -> None:
        array = tensor.detach().float().numpy()
        tensors.append((name, array.tobytes(order="C"), "f32", list(array.shape)))

    def i8(name: str, tensor: torch.Tensor) -> None:
        packed, scale, shape = quantize_into(tensor)
        tensors.append((name, packed, "i8", shape))
        tensors.append((f"{name}.scale", scale, "f32", [shape[0]]))

    f32("emb.weight", model.emb.weight)
    i8("proj.weight", model.proj.weight)
    f32("proj.bias", model.proj.bias)
    for index, layer in enumerate(model.encoder.layers):
        prefix = f"layers.{index}."
        i8(f"{prefix}self_attn.in_proj_weight", layer.self_attn.in_proj_weight)
        f32(f"{prefix}self_attn.in_proj_bias", layer.self_attn.in_proj_bias)
        i8(f"{prefix}self_attn.out_proj.weight", layer.self_attn.out_proj.weight)
        f32(f"{prefix}self_attn.out_proj.bias", layer.self_attn.out_proj.bias)
        i8(f"{prefix}linear1.weight", layer.linear1.weight)
        f32(f"{prefix}linear1.bias", layer.linear1.bias)
        i8(f"{prefix}linear2.weight", layer.linear2.weight)
        f32(f"{prefix}linear2.bias", layer.linear2.bias)
        f32(f"{prefix}norm1.weight", layer.norm1.weight)
        f32(f"{prefix}norm1.bias", layer.norm1.bias)
        f32(f"{prefix}norm2.weight", layer.norm2.weight)
        f32(f"{prefix}norm2.bias", layer.norm2.bias)
    i8("out.weight", model.out.weight)
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
    }
    (OUT / "test-model.golden.json").write_text(
        json.dumps(golden, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {target} {manifest['bytes']} bytes sha256 {manifest['sha256']}")
    print("golden:", golden["top_ids"])


if __name__ == "__main__":
    main()
