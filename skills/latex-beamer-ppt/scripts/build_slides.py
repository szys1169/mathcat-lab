#!/usr/bin/env python3
"""Compile a Beamer deck with latexmk (xelatex) and report errors / overfull boxes."""

import argparse
import os
import subprocess
import sys


def find_job(root: str):
    for name in ("slides.tex", "main.tex"):
        if os.path.isfile(os.path.join(root, name)):
            return name
    texs = sorted(f for f in os.listdir(root) if f.endswith(".tex"))
    return texs[0] if len(texs) == 1 else None


def parse_log(log_path: str):
    errors = []
    overfull = []
    try:
        with open(log_path, encoding="utf-8", errors="replace") as fh:
            lines = fh.readlines()
    except FileNotFoundError:
        return errors, overfull
    for i, line in enumerate(lines):
        if line.startswith("!"):
            errors.append("".join(lines[i : i + 12]).strip())
        if "Overfull" in line or "Underfull" in line:
            overfull.append(line.strip())
    return errors, overfull


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=".", help="directory containing the .tex file")
    ap.add_argument("--job", default=None, help=".tex filename (default: slides.tex / main.tex / single .tex)")
    ap.add_argument("--engine", default="xelatex", choices=("xelatex", "pdflatex"))
    ap.add_argument("--clean", action="store_true", help="run latexmk -c after a successful build")
    ap.add_argument("--strict-overfull", action="store_true", help="fail when the log contains Overfull boxes")
    args = ap.parse_args()

    root = os.path.abspath(args.root)
    if not os.path.isdir(root):
        print(f"[ERROR] root directory not found: {root}")
        return 2

    job = args.job or find_job(root)
    if not job:
        print(f"[ERROR] no .tex file found in {root} (expected slides.tex or main.tex)")
        return 2

    engine_flag = {"xelatex": "-xelatex", "pdflatex": "-pdf"}[args.engine]
    cmd = [
        "latexmk",
        engine_flag,
        "-interaction=nonstopmode",
        "-halt-on-error",
        "-synctex=1",
        job,
    ]
    print(f"[BUILD] cd {root} && {' '.join(cmd)}")
    proc = subprocess.run(cmd, cwd=root)

    base = os.path.splitext(job)[0]
    log_path = os.path.join(root, base + ".log")
    errors, overfull = parse_log(log_path)

    if proc.returncode != 0:
        print(f"[FAIL] compile failed with exit code {proc.returncode}")
        for err in errors[:5]:
            print("-" * 40)
            print(err)
        return 1

    print(f"[OK]   {base}.pdf produced")
    overfull_only = [line for line in overfull if "Overfull" in line]
    if overfull:
        print(f"[WARN] {len(overfull)} box warning(s), including {len(overfull_only)} Overfull:")
        for w in overfull[:10]:
            print("  " + w)
    else:
        print("[OK]   no overfull/underfull box warnings")

    try:
        try:
            import pymupdf as fitz
        except ImportError:
            import fitz

        pdf_path = os.path.join(root, base + ".pdf")
        with fitz.open(pdf_path) as doc:
            print(f"[OK]   {doc.page_count} page(s)")
    except ImportError:
        pass

    if args.strict_overfull and overfull_only:
        print("[FAIL] strict overfull check failed")
        return 1

    if args.clean:
        subprocess.run(["latexmk", "-c", job], cwd=root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
