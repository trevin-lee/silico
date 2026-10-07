"""Compare two probe outputs bit for bit.

Usage: python3 -I compare.py <a.json> <b.json>
"""

import json
import struct
import sys


def as_double(hexbits):
    return struct.unpack(">d", bytes.fromhex(hexbits))[0]


def main(a_path, b_path):
    with open(a_path) as f:
        a = json.load(f)
    with open(b_path) as f:
        b = json.load(f)
    identical = 0
    for case in a:
        ra, rb = a[case], b.get(case)
        if rb is None:
            print(f"{case:24} MISSING in {b_path}")
            continue
        if "error" in ra or "error" in rb:
            same = ra == rb
            identical += same
            print(f"{case:24} {'same error' if same else 'ERROR DIFFERS'}: {ra.get('error')} | {rb.get('error')}")
            continue
        diffs = []
        for key, va in ra.items():
            vb = rb[key]
            if va == vb:
                continue
            if isinstance(va, list):
                x, y = as_double(va[0]), as_double(vb[0])
                scale = max(abs(x), abs(y), 1e-300)
                diffs.append(f"{key} abs {abs(x - y):.1e} rel {abs(x - y) / scale:.1e}")
            else:
                diffs.append(f"{key} hash")
        if diffs:
            print(f"{case:24} DIFFERS: " + ", ".join(diffs))
        else:
            identical += 1
            print(f"{case:24} identical")
    print(f"\n{identical}/{len(a)} cases bit-identical")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
