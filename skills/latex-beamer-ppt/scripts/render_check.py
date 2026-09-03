#!/usr/bin/env python3
"""Render every page of a Beamer PDF to PNG for visual QA."""

import argparse
import glob
import os
import sys


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pdf", help="compiled Beamer PDF")
    ap.add_argument("--out", default="page_images", help="output directory for PNGs")
    ap.add_argument("--dpi", type=int, default=150)
    args = ap.parse_args()

    try:
        import pymupdf as fitz
    except ImportError:
        try:
            import fitz
        except ImportError:
            print("[ERROR] PyMuPDF missing: run  pip install PyMuPDF")
            return 1

    if not os.path.isfile(args.pdf):
        print(f"[ERROR] PDF not found: {args.pdf}")
        return 1

    zoom = args.dpi / 72.0
    matrix = fitz.Matrix(zoom, zoom)
    os.makedirs(args.out, exist_ok=True)
    for stale in glob.glob(os.path.join(args.out, "page_[0-9][0-9][0-9].png")):
        os.remove(stale)
    paths = []

    with fitz.open(args.pdf) as doc:
        for i, page in enumerate(doc, start=1):
            pix = page.get_pixmap(matrix=matrix)
            out = os.path.join(args.out, f"page_{i:03d}.png")
            pix.save(out)
            paths.append(out)

    print(f"[OK]   rendered {len(paths)} page(s) to {os.path.abspath(args.out)}")
    for p in paths:
        print("  " + p)
    return 0


if __name__ == "__main__":
    sys.exit(main())
