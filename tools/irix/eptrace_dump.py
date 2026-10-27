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
from collections import Counter

FMT = {"char": ">b", "unsigned char": ">B", "short": ">h", "unsigned short": ">H",
       "int": ">i", "unsigned int": ">I", "long": ">i", "unsigned long": ">I",
       "float": ">f", "double": ">d"}


def decode(ops, data):
    """Yield (frame, name, args, ret) for each record."""
    assert data[:4] == b"EPT1", "not an eptrace file"
    pos, frame = 4, 0
    while pos + 4 <= len(data):
        op, plen = struct.unpack_from(">HH", data, pos)
        pay = data[pos + 4:pos + 4 + plen]
        pos += 4 + plen
        d = ops[str(op)]
        args, off = [], 0
        for i, p in enumerate(d["params"]):
            ext = d["extents"].get(str(i))
            if ext is not None:
                n = args[int(ext[3:])] if isinstance(ext, str) else ext
                n = n if 0 < n < 512 else 0
                args.append(list(struct.unpack_from(">%df" % n, pay, off)))
                off += 4 * n
            elif p.endswith("*"):
                args.append("0x%08x" % struct.unpack_from(">I", pay, off)[0])
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
        return "%.9g" % v
    if isinstance(v, list):
        return "[" + " ".join(fmt(x) for x in v) + "]"
    if isinstance(v, tuple):
        return "%s=%s" % (v[0], fmt(v[1]))
    return str(v)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ops")
    ap.add_argument("trace")
    ap.add_argument("--frames", default=None, help="A:B frame range (half-open)")
    ap.add_argument("--summary", action="store_true", help="call counts per name")
    a = ap.parse_args()
    ops = json.load(open(a.ops))["ops"]
    data = open(a.trace, "rb").read()
    lo, hi = 0, 1 << 30
    if a.frames:
        x, y = a.frames.split(":")
        lo, hi = int(x or 0), int(y or 1 << 30)
    if a.summary:
        c = Counter()
        frames = 0
        for frame, name, _, _ in decode(ops, data):
            if lo <= frame < hi:
                c[name] += 1
                frames = max(frames, frame)
        print("frames: %d" % (frames + 1))
        for k, v in c.most_common():
            print("%8d %s" % (v, k))
        return
    last = -1
    for frame, name, args, ret in decode(ops, data):
        if not lo <= frame < hi:
            continue
        if frame != last:
            print("== frame %d" % frame)
            last = frame
        s = "%s(%s)" % (name, ", ".join(fmt(x) for x in args))
        print(s if ret is None else "%s = %s" % (s, fmt(ret)))


if __name__ == "__main__":
    main()
