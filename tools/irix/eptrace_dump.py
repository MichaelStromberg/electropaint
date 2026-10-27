#!/usr/bin/env python3
"""Decode an eptrace binary trace (EPT1) into text, one call per line.

Usage: eptrace_dump.py OPS_JSON TRACE [--frames A:B] [--summary]

OPS_JSON is the opcode table written by gen_tracer.py. Values are big-endian
(the trace is written in MIPS native order). Frames are counted by
swapbuffers; a frame's calls are printed under a "== frame N" header.
"""
import argparse
import json
import struct
import sys
from collections import Counter

FMT = {"char": ">b", "unsigned char": ">B", "short": ">h", "unsigned short": ">H",
       "int": ">i", "unsigned int": ">I", "long": ">i", "unsigned long": ">I",
       "float": ">f", "double": ">d"}


def decode(ops, data):
    """Yield (frame, name, args, ret) for each record. A truncated final
    record (the program died mid-write) ends the trace; an opcode missing
    from the table is shown as ?opN with its payload size."""
    if data[:4] != b"EPT1":
        raise ValueError("not an EPT1 trace")
    pos, frame = 4, 0
    while pos + 4 <= len(data):
        op, plen = struct.unpack_from(">HH", data, pos)
        if pos + 4 + plen > len(data):
            print("truncated final record", file=sys.stderr)
            break
        pay = data[pos + 4:pos + 4 + plen]
        pos += 4 + plen
        d = ops.get(str(op))
        if d is None:
            yield frame, f"?op{op}", [f"{plen} bytes"], None
            continue
        args, off = [], 0
        for i, p in enumerate(d["params"]):
            ext = d["extents"].get(str(i))
            if ext is not None:
                n = args[int(ext[3:])] if isinstance(ext, str) else ext
                n = n if 0 < n < 512 else 0
                args.append(list(struct.unpack_from(f">{n}f", pay, off)))
                off += 4 * n
            elif p.endswith("*"):
                args.append(f"0x{struct.unpack_from('>I', pay, off)[0]:08x}")
                off += 4
            else:
                f = FMT[p]
                args.append(struct.unpack_from(f, pay, off)[0])
                off += struct.calcsize(f)
        for _ in d["outs"]:
            if off + 4 <= len(pay):
                args.append(("out", struct.unpack_from(">i", pay, off)[0]))
                off += 4
        ret = None
        if d["ret"] != "void" and off < len(pay):
            f = ">I" if d["ret"].endswith("*") else FMT[d["ret"]]
            ret = struct.unpack_from(f, pay, off)[0]
        yield frame, d["name"], args, ret
        if d["name"] == "swapbuffers":
            frame += 1


def fmt(v):
    if isinstance(v, float):
        return f"{v:.9g}"
    if isinstance(v, list):
        return "[" + " ".join(fmt(x) for x in v) + "]"
    if isinstance(v, tuple):
        return f"{v[0]}={fmt(v[1])}"
    return str(v)


def frame_range(text):
    """`A:B` -> (A, B); A defaults to 0 and B to the end."""
    x, sep, y = text.partition(":")
    try:
        if not sep:
            raise ValueError
        return int(x or 0), int(y or 1 << 30)
    except ValueError:
        raise argparse.ArgumentTypeError("takes A:B (either may be left out)") from None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ops")
    ap.add_argument("trace")
    ap.add_argument("--frames", type=frame_range, default=(0, 1 << 30),
                    help="A:B frame range (half-open; either end may be left out)")
    ap.add_argument("--summary", action="store_true", help="call counts per name")
    a = ap.parse_args()
    with open(a.ops) as f:
        table = json.load(f)
    if table.get("magic") != "EPT1":
        sys.exit(f"{a.ops}: not an EPT1 opcode table")
    ops = table["ops"]
    with open(a.trace, "rb") as f:
        data = f.read()
    lo, hi = a.frames
    try:
        dump(ops, data, lo, hi, a.summary)
    except ValueError as e:
        sys.exit(f"{a.trace}: {e}")


def dump(ops, data, lo, hi, summary):
    """Prints the calls of frames lo..hi, or how often each was called."""
    if summary:
        c = Counter()
        frames = set()
        for frame, name, _, _ in decode(ops, data):
            if lo <= frame < hi:
                c[name] += 1
                frames.add(frame)
        print(f"frames: {len(frames)}")
        for k, v in c.most_common():
            print(f"{v:8d} {k}")
        return
    last = -1
    for frame, name, args, ret in decode(ops, data):
        if not lo <= frame < hi:
            continue
        if frame != last:
            print(f"== frame {frame}")
            last = frame
        s = f"{name}({', '.join(fmt(x) for x in args)})"
        print(s if ret is None else f"{s} = {fmt(ret)}")


if __name__ == "__main__":
    main()
