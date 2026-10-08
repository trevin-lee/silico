"""Compare two stress outputs bit for bit and summarize per operation kind.

Usage: python3 -I compare-stress.py <a.json> <b.json>
"""

import collections
import json
import struct
import sys

KINDS = ["boolean", "rotated boolean", "fillet", "chamfer", "shell", "offset",
         "revolve", "bspline prism", "loft", "pipe sweep", "mesh"]


def as_double(h):
    return struct.unpack(">d", bytes.fromhex(h))[0]


def main(a_path, b_path):
    a = json.load(open(a_path))
    b = json.load(open(b_path))
    per_kind = collections.defaultdict(lambda: [0, 0, 0])  # identical, differ, errors-in-both
    worst = []
    for case, ra in a.items():
        kind = int(case.split("kind")[1])
        rb = b[case]
        if ra == rb:
            per_kind[kind][0] += 1
            if "error" in ra:
                per_kind[kind][2] += 1
            continue
        per_kind[kind][1] += 1
        detail = []
        if "error" in ra or "error" in rb:
            detail.append(f"error A={ra.get('error')!r} B={rb.get('error')!r}")
        else:
            for k, va in ra.items():
                if va != rb[k]:
                    if isinstance(va, list):
                        x, y = as_double(va[0]), as_double(rb[k][0])
                        detail.append(f"{k} abs {abs(x - y):.1e}")
                    else:
                        detail.append(k)
        worst.append(f"  {case}: " + ", ".join(detail))
    total_same = sum(v[0] for v in per_kind.values())
    for k in sorted(per_kind):
        same, diff, errs = per_kind[k]
        print(f"{KINDS[k]:16} {same:3}/{same + diff:<3} identical  ({errs} identical failures)")
    print(f"\nTOTAL: {total_same}/{len(a)} cases bit-identical")
    if worst:
        print("\nDifferences:")
        print("\n".join(worst[:30]))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
