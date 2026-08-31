#!/usr/bin/env python3
"""Measure structure out of window captures, so a UI comparison is numbers rather than impressions.

Capture window-scoped and native:  screencapture -x -o -l <windowid> out.png
That contains the window's own pixels and nothing else, so the wallpaper and the desktop cannot
leak into a reading — a stronger guarantee than setting the desktop to something plain. `-o` omits
the drop shadow, so the frame is exact; do not use it if the shadow is the subject.

Reports per image: full-width horizontal boundaries (toolbar height, header height), the dominant
row pitch by autocorrelation, and the vertical edges inside a band (column positions).
"""

import sys

import numpy as np
from PIL import Image


def load(path):
    im = Image.open(path).convert("RGB")
    return np.asarray(im).astype(float), im.size


def full_width_steps(a, frac=0.55, threshold=10.0):
    """y where a step spans most of the width. Real UI boundaries are full-width lines; text
    antialiasing is not, which is what a plain row-mean threshold picks up instead."""
    luma = a.mean(axis=2)
    d = np.abs(np.diff(luma, axis=0))
    spanning = (d > threshold).mean(axis=1)
    return [int(i) + 1 for i in np.nonzero(spanning > frac)[0]], luma.mean(axis=1)


def pitch(luma, y0, y1):
    """Dominant period of the alternating row bands, by autocorrelation. Reads the repeat directly
    and does not care how many boundaries were missed or invented."""
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
    """x where the band's column-mean luma steps. Column separators and text starts land here."""
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

    # The list starts after the last boundary in the top third; above it is chrome.
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
