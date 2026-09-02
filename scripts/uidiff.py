#!/usr/bin/env python3
"""Report structure measured out of window captures: full-width horizontal boundaries, the
dominant row pitch, and the column edges of the header band.

Capture with: screencapture -x -o -l <windowid> out.png
"""

import sys

import numpy as np
from PIL import Image


def load(path):
    im = Image.open(path).convert("RGB")
    return np.asarray(im).astype(float), im.size


def full_width_steps(a, frac=0.55, threshold=10.0):
    """y where a step spans most of the width; text antialiasing does not."""
    luma = a.mean(axis=2)
    d = np.abs(np.diff(luma, axis=0))
    spanning = (d > threshold).mean(axis=1)
    return [int(i) + 1 for i in np.nonzero(spanning > frac)[0]], luma.mean(axis=1)


def pitch(luma, y0, y1):
    """Dominant period of the alternating row bands, by autocorrelation."""
    seg = luma[y0:y1]
    if len(seg) < 40:
        return None, 0.0
    seg = seg - seg.mean()
    if not seg.any():
        return None, 0.0
    ac = np.correlate(seg, seg, mode="full")[len(seg) - 1 :]
    ac /= ac[0]
    lo, hi = 12, min(160, len(ac) - 1)
    if hi <= lo:
        return None, 0.0
    k = int(np.argmax(ac[lo:hi])) + lo
    return k, float(ac[k])


def col_edges(a, y0, y1, threshold=8.0):
    """x where the band's column-mean luma steps."""
    band = a[y0:y1].mean(axis=2).mean(axis=0)
    d = np.abs(np.diff(band))
    return [int(i) + 1 for i in np.nonzero(d > threshold)[0]]


def describe(path, scale):
    a, size = load(path)
    print(f"\n{path}")
    print(f"  {size[0]}x{size[1]} px @ {scale}x  ->  {size[0]/scale:.0f}x{size[1]/scale:.0f} pt")

    steps, luma = full_width_steps(a)
    if not steps:
        print("  no full-width boundary found — flat image, treat as a failed capture")
        return
    pt = lambda v: round(v / scale, 1)
    print(f"  full-width boundaries (pt): {[pt(s) for s in steps[:14]]}")

    top_third = [s for s in steps if s < size[1] / 3]
    list_top = top_third[-1] if top_third else steps[0]
    print(f"  chrome ends / list starts at: {pt(list_top)} pt")

    k, strength = pitch(luma, list_top, size[1])
    if k:
        print(f"  row pitch: {k} px = {pt(k)} pt  (autocorrelation {strength:.2f})")
        if strength < 0.25:
            print("    weak — the list may not have alternating bands, so treat as unmeasured")

    hdr = max(0, list_top - int(22 * scale))
    print(f"  column edges in the header band (pt): {[pt(c) for c in col_edges(a, hdr, list_top)][:12]}")


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        print("usage: uidiff.py <capture.png> [...] [--scale N]")
        return 1
    scale, paths, i = 2.0, [], 0
    while i < len(args):
        if args[i] == "--scale":
            scale = float(args[i + 1])
            i += 2
        else:
            paths.append(args[i])
            i += 1
    for p in paths:
        describe(p, scale)
    return 0


if __name__ == "__main__":
    sys.exit(main())
