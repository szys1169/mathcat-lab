#!/usr/bin/env python3
"""Validate the observable delivery contract for a generated Beamer/PPTX deck."""

import argparse
import json
import os
import re
import sys


REQUIRED_FILES = (
    "slides.tex",
    "slides.pdf",
    "speaker_notes.md",
    "final.pptx",
    "slide_source_ledger.md",
    "ppt-task-report.md",
)


def note_numbers(path):
    with open(path, encoding="utf-8") as handle:
        return [int(value) for value in re.findall(r"^##\s*Slide\s+(\d+)\s*$", handle.read(), re.MULTILINE)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("root", help="PPT task output directory")
    parser.add_argument("--json", action="store_true", help="print machine-readable result")
    args = parser.parse_args()

    root = os.path.abspath(args.root)
    errors = []
    for name in REQUIRED_FILES:
        target = os.path.join(root, name)
        if not os.path.isfile(target) or os.path.getsize(target) == 0:
            errors.append(f"missing or empty: {name}")

    pdf_pages = pptx_pages = None
    pdf_path = os.path.join(root, "slides.pdf")
    pptx_path = os.path.join(root, "final.pptx")
    try:
        import pymupdf as fitz
    except ImportError:
        import fitz
    if os.path.isfile(pdf_path):
        with fitz.open(pdf_path) as document:
            pdf_pages = document.page_count
        if pdf_pages == 0:
            errors.append("slides.pdf has no pages")

    if os.path.isfile(pptx_path):
        from pptx import Presentation
        pptx_pages = len(Presentation(pptx_path).slides)
        if pdf_pages is not None and pptx_pages != pdf_pages:
            errors.append(f"PPTX pages {pptx_pages} != PDF pages {pdf_pages}")

    images_dir = os.path.join(root, "page_images")
    image_count = len([
        name for name in os.listdir(images_dir)
        if re.fullmatch(r"page_\d{3}\.png", name)
    ]) if os.path.isdir(images_dir) else 0
    if pdf_pages is not None and image_count != pdf_pages:
        errors.append(f"rendered pages {image_count} != PDF pages {pdf_pages}")

    notes_path = os.path.join(root, "speaker_notes.md")
    notes = note_numbers(notes_path) if os.path.isfile(notes_path) else []
    if pdf_pages is not None and notes != list(range(1, pdf_pages + 1)):
        errors.append("speaker notes must contain one ordered section for every slide")

    result = {
        "ok": not errors,
        "root": root,
        "pdfPages": pdf_pages,
        "pptxPages": pptx_pages,
        "renderedPages": image_count,
        "errors": errors,
    }
    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        print("[OK] delivery contract passed" if not errors else "[FAIL] delivery contract failed")
        for error in errors:
            print("  - " + error)
    return 0 if not errors else 1


if __name__ == "__main__":
    sys.exit(main())
