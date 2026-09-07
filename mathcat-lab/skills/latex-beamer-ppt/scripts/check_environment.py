#!/usr/bin/env python3
"""Check the LaTeX and Python toolchain required by latex-beamer-ppt."""

import importlib.util
import shutil
import sys


REQUIRED_BINS = ("latexmk", "xelatex")
REQUIRED_MODULES = (("pptx", "python-pptx"), ("fitz", "PyMuPDF"))


def main() -> int:
    ok = True

    print("== LaTeX ==")
    for name in REQUIRED_BINS:
        path = shutil.which(name)
        if path:
            print(f"[OK]   {name}: {path}")
        else:
            ok = False
            print(f"[MISS] {name}: not found on PATH (install MiKTeX or TeX Live)")

    print("== Python packages ==")
    for mod, pkg in REQUIRED_MODULES:
        if importlib.util.find_spec(mod):
            print(f"[OK]   {pkg}")
        else:
            ok = False
            print(f"[MISS] {pkg}: run  pip install {pkg}")

    if not ok:
        print(
            "\nMissing items above. "
            "LaTeX: install MiKTeX/TeX Live; "
            "Python: pip install PyMuPDF python-pptx"
        )
        return 1

    print("\nEnvironment OK.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
