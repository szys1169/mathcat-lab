"""Standalone CLI adapter; delegates all behavior to the shared package."""

from pathlib import Path
import sys

PACKAGE_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PACKAGE_ROOT))

from math_paper_writing.cli import main  # noqa: E402


if __name__ == "__main__":
    raise SystemExit(main())
