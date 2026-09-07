#!/usr/bin/env python3
"""Render every page of a Beamer PDF to PNG for visual QA."""

import argparse
import glob
import hashlib
import json
import os
import sys


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def text_layout_warnings(page, page_number):
    """Return conservative warnings for clipped or strongly overlapping text blocks."""
    warnings = []
    bounds = page.rect
    blocks = []
    for block in page.get_text("blocks"):
        rect = type(bounds)(block[:4])
        text = block[4].strip()
        block_type = block[6] if len(block) > 6 else 0
        if block_type == 0 and text:
            blocks.append((rect, " ".join(text.split())[:120]))
            if rect.x0 < bounds.x0 - 1 or rect.y0 < bounds.y0 - 1 or rect.x1 > bounds.x1 + 1 or rect.y1 > bounds.y1 + 1:
                warnings.append({"page": page_number, "type": "text_outside_page", "text": text[:120]})
    for index, (left, left_text) in enumerate(blocks):
        for right, right_text in blocks[index + 1:]:
            # PDF math extraction often emits subscripts or fractions as tiny
            # standalone blocks. They are not reliable overlap evidence.
            if min(len(left_text), len(right_text)) < 6:
                continue
            intersection = left & right
            if intersection.is_empty:
                continue
            overlap = intersection.get_area() / max(1.0, min(left.get_area(), right.get_area()))
            if overlap >= 0.20:
                warnings.append({
                    "page": page_number,
                    "type": "text_block_overlap",
                    "overlapRatio": round(overlap, 3),
                    "texts": [left_text, right_text],
                })
    return warnings


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pdf", help="compiled Beamer PDF")
    ap.add_argument("--out", default="page_images", help="output directory for PNGs")
    ap.add_argument("--dpi", type=int, default=150)
    ap.add_argument("--report", default=None, help="render evidence JSON (default: beside page_images)")
    ap.add_argument("--strict-layout", action="store_true", help="fail on detected text clipping or strong text-block overlap")
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

    layout_warnings = []
    with fitz.open(args.pdf) as doc:
        page_count = doc.page_count
        for i, page in enumerate(doc, start=1):
            layout_warnings.extend(text_layout_warnings(page, i))
            pix = page.get_pixmap(matrix=matrix)
            out = os.path.join(args.out, f"page_{i:03d}.png")
            pix.save(out)
            paths.append(out)

    report_path = args.report or os.path.join(os.path.dirname(os.path.abspath(args.out)), "render_report.json")
    report = {
        "schemaVersion": "1.0",
        "status": "passed" if len(paths) == page_count and page_count > 0 and not (args.strict_layout and layout_warnings) else "failed",
        "pdf": os.path.abspath(args.pdf),
        "pdfSha256": sha256(args.pdf),
        "pdfPages": page_count,
        "renderedPages": len(paths),
        "imagesRetained": True,
        "pageImages": [os.path.basename(path) for path in paths],
        "layoutWarnings": layout_warnings,
        "strictLayout": args.strict_layout,
    }
    with open(report_path, "w", encoding="utf-8") as handle:
        json.dump(report, handle, ensure_ascii=False, indent=2)
        handle.write("\n")

    print(f"[OK]   rendered {len(paths)} page(s) to {os.path.abspath(args.out)}")
    print(f"[OK]   render report: {os.path.abspath(report_path)}")
    for warning in layout_warnings:
        print(f"[WARN] page {warning['page']}: {warning['type']}")
    for p in paths:
        print("  " + p)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
