#!/usr/bin/env python3
"""Export a Beamer PDF to PPTX: each page as a full-slide image, speaker notes in remarks."""

import argparse
import json
import os
import re
import shutil
import sys


def parse_notes_md(path: str):
    """Parse '## Slide N' sections into {N: notes_text}."""
    notes = {}
    current = None
    buf = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            m = re.match(r"^##\s*Slide\s+(\d+)\s*$", line.strip())
            if m:
                if current is not None:
                    notes[current] = "\n".join(buf).strip()
                current = int(m.group(1))
                buf = []
            elif current is not None:
                buf.append(line)
    if current is not None:
        notes[current] = "\n".join(buf).strip()
    return notes


def parse_notes_json(path: str):
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
    notes = {}
    for i, item in enumerate(data.get("slides", []), start=1):
        text = item.get("notes") or item.get("speaker_notes") or ""
        if text:
            notes[i] = text
    return notes


def load_notes(path: str):
    if path is None:
        return {}
    if path.lower().endswith(".json"):
        return parse_notes_json(path)
    return parse_notes_md(path)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("pdf", help="compiled Beamer PDF")
    ap.add_argument("--out", default="final.pptx")
    ap.add_argument("--notes", default=None, help="speaker_notes.md or speaker_notes.json")
    ap.add_argument("--dpi", type=int, default=192)
    ap.add_argument("--slide-width", type=float, default=13.333, help="PPTX width in inches")
    ap.add_argument("--strict-notes", action="store_true", help="fail unless notes cover every PDF page exactly once")
    args = ap.parse_args()

    try:
        import pymupdf as fitz
    except ImportError:
        try:
            import fitz
        except ImportError:
            print("[ERROR] PyMuPDF missing: run  pip install PyMuPDF")
            return 1
    try:
        from pptx import Presentation
        from pptx.util import Emu, Inches
    except ImportError:
        print("[ERROR] python-pptx missing: run  pip install python-pptx")
        return 1

    if not os.path.isfile(args.pdf):
        print(f"[ERROR] PDF not found: {args.pdf}")
        return 1

    notes = load_notes(args.notes)
    zoom = args.dpi / 72.0
    matrix = fitz.Matrix(zoom, zoom)

    prs = Presentation()
    out_dir = os.path.dirname(os.path.abspath(args.out)) or "."
    render_dir = os.path.join(out_dir, "pptx_pages")
    os.makedirs(render_dir, exist_ok=True)
    rendered = []

    with fitz.open(args.pdf) as doc:
        if doc.page_count == 0:
            print("[ERROR] PDF contains no pages")
            return 1
        page_count = doc.page_count
        page_rect = doc[0].rect
        width_in = args.slide_width
        height_in = width_in * (page_rect.height / page_rect.width)
        prs.slide_width = Inches(width_in)
        prs.slide_height = Inches(height_in)
        blank = prs.slide_layouts[6]

        for i, page in enumerate(doc, start=1):
            pix = page.get_pixmap(matrix=matrix)
            img = os.path.join(render_dir, f"slide_{i:03d}.png")
            pix.save(img)
            rendered.append(img)

    try:
        for i, img in enumerate(rendered, start=1):
            slide = prs.slides.add_slide(blank)
            slide.shapes.add_picture(img, 0, 0, width=prs.slide_width, height=prs.slide_height)
            text = notes.get(i, "")
            if text:
                slide.notes_slide.notes_text_frame.text = text

        out = args.out
        missing_notes = [n for n in range(1, page_count + 1) if not notes.get(n, "").strip()]
        extra_notes = [n for n in sorted(notes) if not 1 <= n <= page_count]
        if args.strict_notes and (missing_notes or extra_notes):
            print(f"[ERROR] notes mismatch: missing={missing_notes}, extra={extra_notes}")
            return 1

        prs.save(out)
        print(f"[OK]   exported {len(rendered)} slide(s) to {os.path.abspath(out)} "
              f"({width_in:.2f} x {height_in:.2f} in)")

        if notes:
            if extra_notes:
                print(f"[WARN] notes reference non-existent slide numbers: {extra_notes}")
            if len(notes) != len(rendered):
                print(f"[WARN] notes cover {len(notes)} page(s) but PDF has {len(rendered)} page(s); align before delivery")
    finally:
        shutil.rmtree(render_dir, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
