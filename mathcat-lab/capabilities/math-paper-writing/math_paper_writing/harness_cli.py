from __future__ import annotations

import argparse
import json
from pathlib import Path

from .core import CapabilityError, finalize, health, preflight, prepare, terminate

PACKAGE_ROOT = Path(__file__).resolve().parents[1]


def _emit(value: dict) -> None:
    print(json.dumps(value, ensure_ascii=True, separators=(",", ":")))


def main() -> int:
    parser = argparse.ArgumentParser(prog="math-paper-writing-harness")
    subs = parser.add_subparsers(dest="operation", required=True)
    subs.add_parser("health")
    check = subs.add_parser("preflight")
    check.add_argument("--request", required=True)
    prep = subs.add_parser("prepare")
    prep.add_argument("--request", required=True)
    finish = subs.add_parser("finalize")
    finish.add_argument("--run-dir", required=True)
    finish.add_argument("--no-publish-current", action="store_true")
    stop = subs.add_parser("terminate")
    stop.add_argument("--run-dir", required=True)
    stop.add_argument("--reason", required=True, choices=["executor_failed", "user_stopped", "time_limit"])
    stop.add_argument("--message", default="")
    args = parser.parse_args()
    try:
        if args.operation == "health": result = health(PACKAGE_ROOT)
        elif args.operation == "preflight": result = preflight(args.request)
        elif args.operation == "prepare": result = prepare(args.request, PACKAGE_ROOT / "prompts" / "writing.md")
        elif args.operation == "finalize": result = finalize(args.run_dir, publish_current=False if args.no_publish_current else None)
        else: result = terminate(args.run_dir, args.reason, args.message)
    except (CapabilityError, OSError, TimeoutError) as exc:
        _emit({"protocol_version": "1.0", "capability": "math-paper-writing", "ok": False, "error": {"code": "CAPABILITY_ERROR", "message": str(exc)}})
        return 2
    _emit({"protocol_version": "1.0", "capability": "math-paper-writing", "ok": True, "operation": args.operation, "data": result})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
