"""Shared writer for the `OKNM` v1 container.

Used by the exporter and the test-fixture builder so both produce byte-compatible
files:

    magic "OKNM" | u16 version | u32 header_len | header JSON | pad to 64
    vocabulary blob (UTF-8 words, newline separated, id order)
    tensor blocks, each aligned to 64 bytes
"""

import hashlib
import json
import struct

MAGIC = b"OKNM"
VERSION = 1
ALIGN = 64


def align(position: int) -> int:
    return (position + ALIGN - 1) // ALIGN * ALIGN


def quantize_rows(values) -> tuple[bytes, bytes, list[int]]:
    """int8 weights plus one f32 scale per row."""
    import numpy as np

    array = np.asarray(values, dtype=np.float32)
    scale = np.abs(array).max(axis=1) / 127.0
    scale[scale == 0] = 1.0
    quantized = np.clip(np.rint(array / scale[:, None]), -127, 127).astype(np.int8)
    return quantized.tobytes(order="C"), scale.astype(np.float32).tobytes(), list(array.shape)


def write_container(
    path,
    language: str,
    words: list[str],
    tensors: list[tuple[str, bytes, str, list[int]]],
    context: int,
    embed: int,
    d_model: int,
    layers: int,
    heads: int,
    ff: int,
    source: str = "",
) -> dict:
    """tensors: (name, raw bytes, dtype, shape) in the order the reader expects."""
    vocab_blob = "\n".join(words).encode("utf-8")
    header = {
        "schema": 1,
        "format": MAGIC.decode(),
        "version": VERSION,
        "language": language,
        "context": context,
        "vocab": len(words),
        "embed": embed,
        "d_model": d_model,
        "layers": layers,
        "heads": heads,
        "ff": ff,
        "dtype": "f32",
        "vocab_offset": 0,
        "vocab_bytes": len(vocab_blob),
        "tensors": [],
    }
    payload = bytearray(vocab_blob)
    for name, raw, dtype, shape in tensors:
        payload += b"\0" * (align(len(payload)) - len(payload))
        header["tensors"].append(
            {
                "name": name,
                "shape": shape,
                "dtype": dtype,
                "offset": len(payload),
                "bytes": len(raw),
            }
        )
        payload += raw

    header_bytes = json.dumps(header, separators=(",", ":")).encode("utf-8")
    prefix = struct.pack("<4sHI", MAGIC, VERSION, len(header_bytes)) + header_bytes
    prefix += b"\0" * (align(len(prefix)) - len(prefix))
    blob = prefix + bytes(payload)
    path.write_bytes(blob)

    digest = hashlib.sha256(blob).hexdigest().upper()
    manifest = {
        "name": path.stem,
        "version": 1,
        "format": "OKNM1",
        "language": language,
        "bytes": len(blob),
        "sha256": digest,
        "source": source,
    }
    path.with_suffix(".manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    return manifest
