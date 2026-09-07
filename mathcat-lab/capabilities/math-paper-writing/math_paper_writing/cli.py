from __future__ import annotations

import argparse
import json
from pathlib import Path

from .core import CapabilityError, finalize, preflight, prepare, terminate


def main() -> int:
    parser = argparse.ArgumentParser(prog="math-paper-writing")
    subparsers = parser.add_subparsers(dest="command", required=True)
    preflight_parser = subparsers.add_parser("preflight")
    preflight_parser.add_argument("--request", required=True)
    prepare_parser = subparsers.add_parser("prepare")
    prepare_parser.add_argument("--request", required=True)
    finalize_parser = subparsers.add_parser("finalize")
    finalize_parser.add_argument("--run-dir", required=True)
    finalize_parser.add_argument("--no-publish-current", action="store_true")
    terminate_parser = subparsers.add_parser("terminate")
    terminate_parser.add_argument("--run-dir", required=True)
    terminate_parser.add_argument("--reason", required=True, choices=["executor_failed", "user_stopped", "time_limit"])
    terminate_parser.add_argument("--message", default="")
    args = parser.parse_args()
    try:
        if args.command == "preflight":
            output = preflight(args.request)
        elif args.command == "prepare":
            instructions = Path(__file__).resolve().parents[1] / "prompts" / "writing.md"
            output = prepare(args.request, instructions)
        elif args.command == "finalize":
            output = finalize(args.run_dir, publish_current=False if args.no_publish_current else None)
        else:
            output = terminate(args.run_dir, args.reason, args.message)
    except (CapabilityError, OSError, TimeoutError) as exc:
        print(json.dumps({"status": "failed", "error": str(exc)}, ensure_ascii=True, indent=2))
        return 2
    print(json.dumps(output, ensure_ascii=True, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
