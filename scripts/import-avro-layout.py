#!/usr/bin/env python3
"""Read a legacy `.nmsLayout` file as data, without unpickling.

The legacy layout files are Python pickle protocol-4 containers. `pickle.load`
is not used here and must not be: resolving a pickle can import and call
arbitrary callables. This module implements a small stack machine for the
subset of opcodes that a data-only container uses, refuses every opcode that
could execute code (GLOBAL, REDUCE, BUILD, OBJ, INST, NEWOBJ, ...), and returns
plain dicts/lists/strings.

Usage:
    python scripts/import-avro-layout.py <file.nmsLayout> [<out.json>]

With two paths it writes the decoded structure as JSON. With one path it prints
a short summary plus every rule row.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

# Opcodes. Names follow the CPython pickle specification.
PROTO = b"\x80"
FRAME = b"\x95"
STOP = b"."
MARK = b"("
EMPTY_TUPLE = b")"
EMPTY_LIST = b"]"
EMPTY_DICT = b"}"
SETITEM = b"s"
SETITEMS = b"u"
APPEND = b"a"
APPENDS = b"e"
TUPLE = b"t"
TUPLE1 = b"\x85"
TUPLE2 = b"\x86"
TUPLE3 = b"\x87"
SHORT_BINUNICODE = b"\x8c"
BINUNICODE = b"X"
MEMOIZE = b"\x94"
BINPUT = b"q"
LONG_BINPUT = b"r"
BINGET = b"h"
LONG_BINGET = b"j"
BININT = b"J"
BININT1 = b"K"
BININT2 = b"M"
NEWFALSE = b"\x89"
NEWTRUE = b"\x88"
NONE = b"N"
BINFLOAT = b"G"
SHORT_BINBYTES = b"C"

# Anything that can import or call code is rejected by omission.
FORBIDDEN = {
    b"c": "GLOBAL",
    b"R": "REDUCE",
    b"b": "BUILD",
    b"o": "OBJ",
    b"i": "INST",
    b"\x81": "NEWOBJ",
    b"\x82": "EXT1",
    b"\x83": "EXT2",
    b"\x84": "EXT4",
    b"\x93": "STACK_GLOBAL",
}

_MAX_DEPTH = 64


class PickleError(ValueError):
    pass


class Reader:
    def __init__(self, data: bytes) -> None:
        self.data = data
        self.pos = 0
        self.stack: list[object] = []
        self.memo: dict[int, object] = {}
        self.mark: list[int] = []

    def take(self, size: int) -> bytes:
        end = self.pos + size
        if end > len(self.data):
            raise PickleError(f"truncated at {self.pos}")
        chunk = self.data[self.pos : end]
        self.pos = end
        return chunk

    def read_uint(self, size: int) -> int:
        return int.from_bytes(self.take(size), "little")

    def pop(self) -> object:
        if not self.stack:
            raise PickleError(f"stack underflow at {self.pos}")
        return self.stack.pop()

    def pop_to_mark(self) -> list[object]:
        if not self.mark:
            raise PickleError(f"no mark at {self.pos}")
        depth = self.mark.pop()
        items = self.stack[depth:]
        del self.stack[depth:]
        return items

    def run(self) -> object:
        while self.pos < len(self.data):
            op = self.data[self.pos : self.pos + 1]
            self.pos += 1
            if op in FORBIDDEN:
                raise PickleError(
                    f"{FORBIDDEN[op]} at {self.pos - 1} would execute code; refusing"
                )
            if op == PROTO:
                self.pos += 1
            elif op == FRAME:
                self.pos += 8
            elif op == SHORT_BINUNICODE:
                length = self.read_uint(1)
                self.stack.append(self.take(length).decode("utf-8"))
            elif op == BINUNICODE:
                length = self.read_uint(4)
                self.stack.append(self.take(length).decode("utf-8"))
            elif op == SHORT_BINBYTES:
                length = self.read_uint(1)
                self.stack.append(self.take(length))
            elif op == EMPTY_DICT:
                self.stack.append({})
            elif op == EMPTY_LIST:
                self.stack.append([])
            elif op == EMPTY_TUPLE:
                self.stack.append(())
            elif op == MARK:
                self.mark.append(len(self.stack))
            elif op == SETITEM:
                value = self.pop()
                key = self.pop()
                target = self.stack[-1]
                if not isinstance(target, dict):
                    raise PickleError(f"SETITEM on {type(target)} at {self.pos}")
                target[key] = value
            elif op == SETITEMS:
                items = self.pop_to_mark()
                target = self.stack[-1]
                if len(items) % 2:
                    raise PickleError(f"odd SETITEMS at {self.pos}")
                for index in range(0, len(items), 2):
                    target[items[index]] = items[index + 1]
            elif op == APPEND:
                value = self.pop()
                self.stack[-1].append(value)
            elif op == APPENDS:
                values = self.pop_to_mark()
                self.stack[-1].extend(values)
            elif op == TUPLE:
                self.stack.append(tuple(self.pop_to_mark()))
            elif op == TUPLE1:
                self.stack.append((self.pop(),))
            elif op == TUPLE2:
                second = self.pop()
                first = self.pop()
                self.stack.append((first, second))
            elif op == TUPLE3:
                third = self.pop()
                second = self.pop()
                first = self.pop()
                self.stack.append((first, second, third))
            elif op == MEMOIZE:
                self.memo[len(self.memo)] = self.stack[-1]
            elif op == BINPUT:
                self.memo[self.read_uint(1)] = self.stack[-1]
            elif op == LONG_BINPUT:
                self.memo[self.read_uint(4)] = self.stack[-1]
            elif op == BINGET:
                self.stack.append(self.memo[self.read_uint(1)])
            elif op == LONG_BINGET:
                self.stack.append(self.memo[self.read_uint(4)])
            elif op == BININT:
                self.stack.append(int.from_bytes(self.take(4), "little", signed=True))
            elif op == BININT1:
                self.stack.append(self.read_uint(1))
            elif op == BININT2:
                self.stack.append(self.read_uint(2))
            elif op == BINFLOAT:
                self.stack.append(struct_unpack(self.take(8)))
            elif op == NEWTRUE:
                self.stack.append(True)
            elif op == NEWFALSE:
                self.stack.append(False)
            elif op == NONE:
                self.stack.append(None)
            elif op == STOP:
                if len(self.stack) != 1:
                    raise PickleError(f"STOP with {len(self.stack)} values")
                return self.stack[0]
            else:
                raise PickleError(
                    f"unsupported opcode {op.hex()} at {self.pos - 1}"
                )
        raise PickleError("missing STOP")


def struct_unpack(raw: bytes) -> float:
    import struct

    return struct.unpack(">d", raw)[0]


def read_layout(path: Path) -> object:
    return Reader(path.read_bytes()).run()


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    layout = read_layout(Path(argv[1]))
    if len(argv) >= 3:
        Path(argv[2]).write_text(
            json.dumps(layout, ensure_ascii=False, indent=2, sort_keys=True),
            encoding="utf-8",
        )
        print(f"wrote {argv[2]}")
        return 0
    if isinstance(layout, dict):
        print("keys:", ", ".join(sorted(layout)))
        for key in sorted(layout):
            value = layout[key]
            if isinstance(value, (list, tuple)):
                print(f"{key}: {len(value)} entries")
            elif isinstance(value, dict):
                print(f"{key}: dict with {len(value)} entries")
            else:
                print(f"{key}: {value!r}")
    else:
        print(type(layout).__name__)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
