#!/usr/bin/env python3
"""Validate the observable delivery contract for a generated Beamer/PPTX deck."""

import argparse
import glob
import hashlib
import json
import os
import re
import shutil
import sys


REQUIRED_FILES = (
    "slides.tex",
    "slides.pdf",
    "speaker_notes.md",
    "final.pptx",
    "slide_source_ledger.md",
    "ppt-task-report.md",
    "render_report.json",
)


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def note_numbers(path):
    with open(path, encoding="utf-8") as handle:
        return [int(value) for value in re.findall(r"^##\s*Slide\s+(\d+)\s*$", handle.read(), re.MULTILINE)]


def note_metadata_errors(path):
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    sections = re.split(r"(?=^##\s*Slide\s+\d+\s*$)", text, flags=re.MULTILINE)
    errors = []
    for section in sections:
        match = re.match(r"^##\s*Slide\s+(\d+)\s*$", section, re.MULTILINE)
        if not match:
            continue
        number = match.group(1)
        for label in ("预计用时", "必须讲", "过渡句", "[Sources]"):
            if label not in section:
                errors.append(f"speaker note Slide {number} missing {label}")
    return errors


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("root", help="PPT task output directory")
    parser.add_argument("--json", action="store_true", help="print machine-readable result")
    parser.add_argument("--cleanup-page-images", action="store_true", help="remove successful QA PNGs after validation")
    parser.add_argument("--strict-notes-metadata", action="store_true", help="require timing, key point, transition, and sources in every note")
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
    render_report_path = os.path.join(root, "render_report.json")
    render_report = None
    if os.path.isfile(render_report_path):
        try:
            with open(render_report_path, encoding="utf-8") as handle:
                render_report = json.load(handle)
        except (OSError, json.JSONDecodeError):
            errors.append("render_report.json is unreadable")
    report_valid = bool(
        render_report
        and render_report.get("status") == "passed"
        and render_report.get("pdfPages") == pdf_pages
        and os.path.isfile(pdf_path)
        and render_report.get("pdfSha256") == sha256(pdf_path)
    )
    if image_count not in (0, pdf_pages):
        errors.append(f"rendered pages {image_count} != PDF pages {pdf_pages}")
    if image_count == 0 and not report_valid:
        errors.append("page images are absent and render_report.json does not verify the current PDF")

    notes_path = os.path.join(root, "speaker_notes.md")
    notes = note_numbers(notes_path) if os.path.isfile(notes_path) else []
    if pdf_pages is not None and notes != list(range(1, pdf_pages + 1)):
        errors.append("speaker notes must contain one ordered section for every slide")
    if args.strict_notes_metadata and os.path.isfile(notes_path):
        errors.extend(note_metadata_errors(notes_path))

    if not errors and args.cleanup_page_images and image_count:
        for image in glob.glob(os.path.join(images_dir, "page_[0-9][0-9][0-9].png")):
            os.remove(image)
        try:
            os.rmdir(images_dir)
        except OSError:
            pass
        image_count = 0
        render_report["imagesRetained"] = False
        with open(render_report_path, "w", encoding="utf-8") as handle:
            json.dump(render_report, handle, ensure_ascii=False, indent=2)
            handle.write("\n")

    result = {
        "ok": not errors,
        "root": root,
        "pdfPages": pdf_pages,
        "pptxPages": pptx_pages,
        "renderedPages": image_count,
        "renderEvidence": "render_report.json" if report_valid else None,
        "pageImagesRetained": bool(image_count),
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
