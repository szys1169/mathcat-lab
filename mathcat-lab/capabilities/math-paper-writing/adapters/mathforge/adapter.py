"""Thin Python API used by the future MathForge Capability Registry."""

from pathlib import Path
import sys

PACKAGE_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PACKAGE_ROOT))

from math_paper_writing.core import finalize, preflight, prepare, terminate  # noqa: E402


def preflight_task(request_path: str) -> dict:
    return preflight(request_path)


def prepare_task(request_path: str) -> dict:
    return prepare(request_path, PACKAGE_ROOT / "prompts" / "writing.md")


def finalize_task(run_dir: str) -> dict:
    return finalize(run_dir)


def terminate_task(run_dir: str, reason: str, message: str = "") -> dict:
    return terminate(run_dir, reason, message)
