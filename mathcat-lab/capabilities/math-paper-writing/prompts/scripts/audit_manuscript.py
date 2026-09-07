#!/usr/bin/env python3
"""Mechanical manuscript lint for math-paper-writing.

This script locates review candidates. It does not decide whether a proof is
complete or whether a mathematical statement is correct. By default findings
do not change the exit status; pass --strict to return 1 when findings exist.
"""

from __future__ import annotations

import argparse
import json
import re
from collections import Counter
from pathlib import Path
from typing import Iterable


PROCESS_PATTERNS = {
    "zh-obvious": r"显然|容易看出|不难看出|标准论证",
    "zh-revision-trace": r"按(?:你的|用户|作者)?要求|需要说明|这里我们修改|上一版|不再尝试",
    "en-obvious": r"\b(?:obviously|clearly|it is easy to see|by a standard argument)\b",
    "en-revision-trace": r"\b(?:as requested|in the previous version|we now revise|we no longer try)\b",
    "priority-language": r"\b(?:to (?:the best of )?our knowledge|for the first time|we are the first|novel|new (?:result|theorem|proof|method|bound)|strictly stronger|optimal|sharp|breakthrough)\b|首次|首创|远强于|严格更强|最优|尖锐",
    "internal-path": r"(?:[A-Za-z]:\\(?:Users|research|Documents|Desktop|temp|tmp|workspace)\\|/(?:home|Users|tmp)/|(?:^|[\s`])(?:writer|output|tmp)/)",
}

THEOREM_ENVS = (
    "theorem",
    "lemma",
    "proposition",
    "corollary",
    "definition",
    "conjecture",
    "remark",
    "example",
    "定理",
    "引理",
    "命题",
    "推论",
    "定义",
    "猜想",
    "注",
    "示例",
)

RESULT_ENVS = ("theorem", "lemma", "proposition", "corollary", "定理", "引理", "命题", "推论")

SECTION_PATTERN = re.compile(
    r"\\section\*?\{(?P<title>[^}]*)\}(?P<body>.*?)(?=\\section\*?\{|\\end\{document\})",
    re.DOTALL | re.IGNORECASE,
)


def prose_words(text: str) -> list[str]:
    """Return rough prose tokens after removing common LaTeX structure."""
    text = re.sub(r"(?m)(?<!\\)%.*$", " ", text)
    text = re.sub(r"\\begin\{[^}]+\}|\\end\{[^}]+\}", " ", text)
    text = re.sub(r"\\(?:cite\w*|ref|eqref|cref|Cref|label)\*?(?:\[[^]]*\])?\{[^}]*\}", " ", text)
    text = re.sub(r"\\[A-Za-z@]+\*?(?:\[[^]]*\])?", " ", text)
    text = re.sub(r"\$\$.*?\$\$|\\\[.*?\\\]|\$.*?\$", " ", text, flags=re.DOTALL)
    text = re.sub(r"[{}~_^&]", " ", text)
    return re.findall(r"[A-Za-z]+(?:[-'][A-Za-z]+)*|[\u4e00-\u9fff]", text)


def introduction_body(text: str) -> str:
    for match in SECTION_PATTERN.finditer(text):
        if re.search(r"\bintroduction\b|引言|绪论", match.group("title"), re.I):
            return match.group("body")
    return ""


def cite_keys(text: str) -> set[str]:
    keys: set[str] = set()
    for group in re.findall(r"\\cite\w*(?:\[[^]]*\])?\{([^}]+)\}", text):
        keys.update(key.strip() for key in group.split(",") if key.strip())
    return keys


def narrative_candidates(path: Path, text: str) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Report narrative metrics and candidates without treating them as errors."""
    findings: list[dict[str, object]] = []
    intro = introduction_body(text)
    intro_cite_commands = len(re.findall(r"\\cite\w*(?:\[[^]]*\])?\{[^}]+\}", intro))
    intro_keys = cite_keys(intro)

    boxed_matches = list(re.finditer(r"\\boxed\s*\{", text))
    for match in boxed_matches:
        findings.append(
            {
                "file": str(path),
                "line": line_number(text, match.start()),
                "kind": "boxed-expression-review",
                "text": r"\boxed{",
            }
        )

    env_names = "|".join(re.escape(env) for env in THEOREM_ENVS)
    block_pattern = re.compile(
        rf"\\begin\{{(?P<env>{env_names})\}}.*?\\end\{{(?P=env)\}}",
        re.DOTALL,
    )
    blocks = list(block_pattern.finditer(text))
    result_blocks = [block for block in blocks if block.group("env") in RESULT_ENVS]
    bridge_word_counts: list[int] = []
    for previous, current in zip(result_blocks, result_blocks[1:]):
        between = text[previous.end() : current.start()]
        count = len(prose_words(between))
        bridge_word_counts.append(count)
        if count < 12 and not re.search(r"\\(?:section|subsection|subsubsection)\*?\{", between):
            findings.append(
                {
                    "file": str(path),
                    "line": line_number(text, current.start()),
                    "kind": "thin-theorem-bridge-candidate",
                    "text": f"{previous.group('env')} -> {current.group('env')}: {count} prose words",
                }
            )

    bibliography_start = re.search(
        r"\\bibliography\{|\\begin\{thebibliography\}|\\printbibliography",
        text,
    )
    terminal_end = bibliography_start.start() if bibliography_start else len(text)
    terminal_start = blocks[-1].end() if blocks else 0
    terminal_words = len(prose_words(text[terminal_start:terminal_end]))
    if blocks and terminal_words < 20:
        findings.append(
            {
                "file": str(path),
                "line": line_number(text, terminal_start),
                "kind": "abrupt-ending-review-candidate",
                "text": f"{terminal_words} prose words after final theorem-like environment",
            }
        )

    metrics = {
        "introduction_words": len(prose_words(intro)),
        "introduction_cite_commands": intro_cite_commands,
        "introduction_unique_citations": len(intro_keys),
        "boxed_expressions": len(boxed_matches),
        "theorem_bridge_word_counts": bridge_word_counts,
        "terminal_prose_words": terminal_words if blocks else None,
    }
    return findings, metrics


def line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def add_findings_for_pattern(
    findings: list[dict[str, object]], path: Path, text: str, kind: str, pattern: str
) -> None:
    for match in re.finditer(pattern, text, flags=re.IGNORECASE | re.MULTILINE):
        findings.append(
            {
                "file": str(path),
                "line": line_number(text, match.start()),
                "kind": kind,
                "text": match.group(0).strip(),
            }
        )


def abstract_body(text: str) -> tuple[str, int] | None:
    match = re.search(
        r"\\begin\{abstract\}(.*?)\\end\{abstract\}", text, flags=re.DOTALL
    )
    if not match:
        return None
    return match.group(1), match.start(1)


def parse_bib_keys(paths: Iterable[Path]) -> set[str]:
    keys: set[str] = set()
    for path in paths:
        if not path.exists():
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        keys.update(re.findall(r"@\w+\s*\{\s*([^,\s]+)", text))
    return keys


def read_tex_tree(path: Path, seen: set[Path] | None = None) -> str:
    r"""Read a root file and resolvable local \input/\include files."""
    path = path.resolve()
    if seen is None:
        seen = set()
    if path in seen or not path.is_file():
        return ""
    seen.add(path)
    text = path.read_text(encoding="utf-8", errors="replace")
    parts = [text]
    for target in re.findall(r"\\(?:input|include)\{([^}]+)\}", text):
        child = Path(target)
        if not child.suffix:
            child = child.with_suffix(".tex")
        if not child.is_absolute():
            child = path.parent / child
        parts.append(read_tex_tree(child, seen))
    return "\n".join(parts)


def audit_tex(path: Path, bib_keys: set[str]) -> tuple[list[dict[str, object]], dict[str, object]]:
    text = path.read_text(encoding="utf-8", errors="replace")
    tree_text = read_tex_tree(path)
    findings: list[dict[str, object]] = []

    for kind, pattern in PROCESS_PATTERNS.items():
        add_findings_for_pattern(findings, path, text, kind, pattern)

    narrative_findings, narrative_metrics = narrative_candidates(path, tree_text)
    findings.extend(narrative_findings)

    abstract = abstract_body(text)
    if abstract:
        body, start = abstract
        display = re.search(
            r"\$\$|\\\[|\\begin\{(?:equation\*?|align\*?|gather\*?|multline\*?)\}",
            body,
        )
        if display:
            findings.append(
                {
                    "file": str(path),
                    "line": line_number(text, start + display.start()),
                    "kind": "display-math-in-abstract",
                    "text": display.group(0),
                }
            )

    labels = re.findall(r"\\label\{([^}]+)\}", tree_text)
    label_counts = Counter(labels)
    for label, count in sorted(label_counts.items()):
        if count > 1:
            findings.append(
                {
                    "file": str(path),
                    "line": None,
                    "kind": "duplicate-label",
                    "text": f"{label} ({count} occurrences)",
                }
            )

    refs = set(re.findall(r"\\(?:eqref|ref|cref|Cref)\{([^}]+)\}", tree_text))
    for label in sorted(refs - set(labels)):
        findings.append(
            {
                "file": str(path),
                "line": None,
                "kind": "locally-unresolved-ref",
                "text": label,
            }
        )

    cites = cite_keys(tree_text)
    if bib_keys:
        for key in sorted(cites - bib_keys):
            findings.append(
                {
                    "file": str(path),
                    "line": None,
                    "kind": "unresolved-citation-key",
                    "text": key,
                }
            )

    env_counts = {
        env: len(re.findall(rf"\\begin\{{{re.escape(env)}\}}", tree_text))
        for env in THEOREM_ENVS
    }
    env_counts = {key: value for key, value in env_counts.items() if value}
    summary = {
        "file": str(path),
        "labels": sorted(set(labels)),
        "citations": sorted(cites),
        "environment_counts": env_counts,
        "narrative_metrics": narrative_metrics,
    }
    return findings, summary


def compare_versions(summaries: list[dict[str, object]]) -> list[dict[str, object]]:
    if len(summaries) < 2:
        return []
    findings: list[dict[str, object]] = []
    base = summaries[0]
    base_labels = set(base["labels"])
    base_envs = base["environment_counts"]
    for current in summaries[1:]:
        labels = set(current["labels"])
        if labels != base_labels:
            findings.append(
                {
                    "file": current["file"],
                    "line": None,
                    "kind": "cross-version-label-difference",
                    "text": {
                        "missing_from_current": sorted(base_labels - labels),
                        "extra_in_current": sorted(labels - base_labels),
                        "compared_with": base["file"],
                    },
                }
            )
        if current["environment_counts"] != base_envs:
            findings.append(
                {
                    "file": current["file"],
                    "line": None,
                    "kind": "cross-version-environment-count-difference",
                    "text": {
                        "base": base_envs,
                        "current": current["environment_counts"],
                        "compared_with": base["file"],
                    },
                }
            )
    return findings


def audit_revision_log(path: Path) -> list[dict[str, object]]:
    if not path.exists():
        return []
    text = path.read_text(encoding="utf-8", errors="replace")
    findings: list[dict[str, object]] = []
    for match in re.finditer(r"\b(?:pending|applied|awaiting bilingual sync)\b", text, re.I):
        findings.append(
            {
                "file": str(path),
                "line": line_number(text, match.start()),
                "kind": "open-human-revision-status",
                "text": match.group(0),
            }
        )
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tex", nargs="+", type=Path, help="one or more TeX manuscripts")
    parser.add_argument("--bib", action="append", type=Path, default=[], help="BibTeX file; repeatable")
    parser.add_argument("--revision-log", type=Path, help="optional human_revision_log.md")
    parser.add_argument("--json", action="store_true", help="emit JSON")
    parser.add_argument("--strict", action="store_true", help="return 1 if any candidates are found")
    args = parser.parse_args()

    tex_paths = [path.resolve() for path in args.tex]
    missing = [str(path) for path in tex_paths if not path.is_file()]
    if missing:
        parser.error("missing TeX file(s): " + ", ".join(missing))

    bib_paths = [path.resolve() for path in args.bib]
    if not bib_paths:
        bib_paths = sorted({path for tex in tex_paths for path in tex.parent.glob("*.bib")})
    bib_keys = parse_bib_keys(bib_paths)

    findings: list[dict[str, object]] = []
    summaries: list[dict[str, object]] = []
    for path in tex_paths:
        current_findings, summary = audit_tex(path, bib_keys)
        findings.extend(current_findings)
        summaries.append(summary)
    findings.extend(compare_versions(summaries))
    if args.revision_log:
        findings.extend(audit_revision_log(args.revision_log.resolve()))

    report = {"summaries": summaries, "findings": findings}
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        for summary in summaries:
            print(
                f"{summary['file']}: labels={len(summary['labels'])}, "
                f"citations={len(summary['citations'])}, "
                f"environments={summary['environment_counts']}, "
                f"narrative_metrics={summary['narrative_metrics']}"
            )
        if findings:
            print(f"\nReview candidates: {len(findings)}")
            for item in findings:
                location = item["file"]
                if item["line"]:
                    location += f":{item['line']}"
                print(f"- [{item['kind']}] {location}: {item['text']}")
        else:
            print("\nNo mechanical review candidates found.")

    return 1 if args.strict and findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
