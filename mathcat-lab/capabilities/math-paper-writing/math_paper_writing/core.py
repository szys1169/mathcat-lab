from __future__ import annotations

import hashlib
import json
import re
import runpy
import shutil
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

EXECUTORS = {"codex", "deepseek_harness"}
LANGUAGES = {"en", "zh", "bilingual"}
WORKFLOW_MODES = {"draft", "revision", "bilingual-sync", "submission-audit"}
TERMINATION_REASONS = {"executor_failed", "user_stopped", "time_limit"}


def health(package_root: str | Path) -> dict[str, Any]:
    root = Path(package_root).resolve()
    required = [root / "capability.json", root / "contracts" / "input.schema.json", root / "contracts" / "result.schema.json", root / "contracts" / "artifacts.schema.json", root / "prompts" / "writing.md"]
    missing = [str(path) for path in required if not path.is_file()]
    return {
        "protocol_version": "1.0", "capability": "math-paper-writing", "capability_version": json.loads((root / "capability.json").read_text(encoding="utf-8"))["version"] if (root / "capability.json").is_file() else None,
        "ready": not missing, "operations": ["health", "preflight", "prepare", "finalize", "terminate"],
        "latex": {"latexmk": shutil.which("latexmk"), "xelatex": shutil.which("xelatex"), "pdflatex": shutil.which("pdflatex")},
        "missing_files": missing
    }


class CapabilityError(RuntimeError):
    pass


def _write_json(path: Path, value: Any) -> None:
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _event(run_dir: Path, event: str, **data: Any) -> None:
    row = {"time": datetime.now(timezone.utc).isoformat(), "event": event, **data}
    with (run_dir / "events.jsonl").open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(row, ensure_ascii=False) + "\n")


def _reset_check_directory(parent: Path, directory: Path) -> None:
    """Only reset the owned build/render child, never a model-created link."""
    if directory.is_symlink() or getattr(directory, "is_junction", lambda: False)():
        raise CapabilityError(f"Check output directory cannot be a link: {directory}")
    if not directory.resolve().is_relative_to(parent.resolve()) or directory.resolve() == parent.resolve():
        raise CapabilityError(f"Check output directory escapes its authorized parent: {directory}")
    if directory.exists():
        shutil.rmtree(directory)
    directory.mkdir(parents=True, exist_ok=True)


def load_request(path: str | Path) -> tuple[dict[str, Any], Path, Path]:
    request_path = Path(path).resolve()
    try:
        request = json.loads(request_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CapabilityError(f"Cannot read request JSON: {exc}") from exc
    required = {"schema_version", "task_id", "executor", "workspace", "source_packet", "language"}
    missing = sorted(required - request.keys())
    if missing:
        raise CapabilityError("Missing request fields: " + ", ".join(missing))
    if request["schema_version"] != "1.0":
        raise CapabilityError("Unsupported schema_version; expected 1.0")
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,100}", request["task_id"]):
        raise CapabilityError("task_id must contain only letters, digits, dot, underscore, or hyphen")
    if request["executor"] not in EXECUTORS:
        raise CapabilityError("executor must be codex or deepseek_harness")
    if request["language"] not in LANGUAGES:
        raise CapabilityError("language must be en, zh, or bilingual")
    unknown = set(request) - {
        "schema_version", "task_id", "executor", "workspace", "source_packet",
        "selected_files", "language", "title_hint", "venue", "requirements",
        "time_limit_seconds", "workflow_mode", "idempotency_key", "publish_current"
    }
    if unknown:
        raise CapabilityError("Unknown request fields: " + ", ".join(sorted(unknown)))
    if request.get("workflow_mode", "draft") not in WORKFLOW_MODES:
        raise CapabilityError("Unknown workflow_mode")
    if "idempotency_key" in request and (not isinstance(request["idempotency_key"], str) or not 1 <= len(request["idempotency_key"]) <= 200):
        raise CapabilityError("idempotency_key must be a nonempty string of at most 200 characters")
    if "publish_current" in request and not isinstance(request["publish_current"], bool):
        raise CapabilityError("publish_current must be a boolean")
    limit = request.get("time_limit_seconds", 3600)
    if not isinstance(limit, int) or isinstance(limit, bool) or not 60 <= limit <= 86400:
        raise CapabilityError("time_limit_seconds must be an integer from 60 to 86400")
    workspace = Path(request["workspace"]).expanduser().resolve()
    if not workspace.is_dir():
        raise CapabilityError(f"Workspace does not exist: {workspace}")
    source = Path(request["source_packet"]).expanduser()
    if not source.is_absolute():
        source = workspace / source
    source = source.resolve()
    if not source.is_file():
        raise CapabilityError(f"Source Packet does not exist: {source}")
    try:
        source.relative_to(workspace)
    except ValueError as exc:
        raise CapabilityError("Source Packet must be inside the authorized workspace") from exc
    for item in request.get("selected_files", []):
        selected = (workspace / item).resolve() if not Path(item).is_absolute() else Path(item).resolve()
        try:
            selected.relative_to(workspace)
        except ValueError as exc:
            raise CapabilityError(f"Selected file is outside workspace: {selected}") from exc
        if not selected.is_file():
            raise CapabilityError(f"Selected file does not exist: {selected}")
    return request, workspace, source


def _source_packet_warnings(source: Path) -> list[str]:
    if source.suffix.lower() != ".json":
        return ["Markdown Source Packet accepted in compatibility mode; structured JSON validation was not applied"]
    try:
        packet = json.loads(source.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CapabilityError(f"Cannot read structured Source Packet: {exc}") from exc
    required = {"schema_version", "writing_goal", "core_results", "source_materials", "citation_whitelist", "evidence_gaps"}
    missing = sorted(required - packet.keys())
    if missing:
        raise CapabilityError("Structured Source Packet is missing: " + ", ".join(missing))
    if packet["schema_version"] != "1.0":
        raise CapabilityError("Unsupported Source Packet schema_version; expected 1.0")
    if not isinstance(packet["core_results"], list) or not packet["core_results"]:
        raise CapabilityError("Structured Source Packet requires at least one core result")
    allowed_statuses = {"proved", "audited", "computational_evidence", "conjectural", "open"}
    for index, result in enumerate(packet["core_results"]):
        if not isinstance(result, dict) or not {"id", "statement", "status", "sources"} <= result.keys():
            raise CapabilityError(f"core_results[{index}] is incomplete")
        if result["status"] not in allowed_statuses:
            raise CapabilityError(f"core_results[{index}] has an unsupported status")
    return []


def _selected_paths(request: dict[str, Any], workspace: Path) -> list[Path]:
    return [
        (workspace / item).resolve() if not Path(item).is_absolute() else Path(item).resolve()
        for item in request.get("selected_files", [])
    ]


def _writing_mode(selected: list[Path]) -> str:
    manuscript_names = re.compile(r"(?:article|paper|manuscript|论文|稿)", re.IGNORECASE)
    return "assemble_existing" if any(path.suffix.lower() == ".tex" and manuscript_names.search(path.stem) for path in selected) else "new_draft"


def preflight(request_path: str | Path) -> dict[str, Any]:
    """Validate and classify inputs without creating a version directory."""
    request, workspace, source = load_request(request_path)
    selected = _selected_paths(request, workspace)
    warnings = _source_packet_warnings(source)
    mode = _writing_mode(selected)
    if mode == "assemble_existing":
        warnings.append("An existing TeX manuscript is selected; this run will assemble or improve supplied writing rather than claim a from-scratch draft")
    return {
        "contract_version": "1.0",
        "capability": "math-paper-writing",
        "operation": "preflight",
        "ready": True,
        "workspace": str(workspace),
        "source_packet": str(source),
        "selected_files": [str(path) for path in selected],
        "writing_mode": mode,
        "warnings": warnings,
    }


def prepare(request_path: str | Path, instructions_file: str | Path) -> dict[str, Any]:
    request, workspace, source = load_request(request_path)
    source_warnings = _source_packet_warnings(source)
    selected = _selected_paths(request, workspace)
    writing_mode = _writing_mode(selected)
    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    version = f'{request["task_id"]}-{timestamp}'
    if request.get("idempotency_key"):
        version = request["task_id"] + "-" + hashlib.sha256(request["idempotency_key"].encode()).hexdigest()[:20]
    run_dir = workspace / "论文" / "versions" / version
    writer_dir = run_dir / "writer"
    logs_dir = run_dir / "logs"
    request_fingerprint = hashlib.sha256(json.dumps(request, sort_keys=True, ensure_ascii=False).encode()).hexdigest()
    existing_task = run_dir / "task.json"
    if existing_task.is_file():
        previous = json.loads(existing_task.read_text(encoding="utf-8"))
        if not request.get("idempotency_key") or previous.get("request_fingerprint") != request_fingerprint:
            raise CapabilityError("The idempotency key already identifies different paper inputs")
        if (run_dir / "execution_request.json").is_file():
            return json.loads((run_dir / "execution_request.json").read_text(encoding="utf-8"))
    try:
        writer_dir.mkdir(parents=True, exist_ok=bool(request.get("idempotency_key")))
        logs_dir.mkdir(exist_ok=bool(request.get("idempotency_key")))
    except OSError as exc:
        raise CapabilityError(f"Cannot create version directory {run_dir}: {exc}") from exc
    normalized = dict(request)
    normalized["workspace"] = str(workspace)
    normalized["source_packet"] = str(source)
    normalized["version"] = version
    normalized["created_at"] = datetime.now(timezone.utc).isoformat()
    normalized["source_packet_warnings"] = source_warnings
    normalized["writing_mode"] = writing_mode
    normalized["workflow_mode"] = request.get("workflow_mode", "revision" if writing_mode == "assemble_existing" else "draft")
    normalized["request_fingerprint"] = request_fingerprint
    _write_json(run_dir / "task.json", normalized)
    source_snapshot = run_dir / f"source_packet{source.suffix.lower()}"
    shutil.copy2(source, source_snapshot)
    input_dir = run_dir / "input_files"
    selected_snapshots = []
    input_rows = []
    for index, (item, selected_path) in enumerate(zip(request.get("selected_files", []), selected), start=1):
        input_dir.mkdir(exist_ok=True)
        destination = input_dir / f"{index:03d}_{selected_path.name}"
        shutil.copy2(selected_path, destination)
        selected_snapshots.append(str(destination))
        input_rows.append({
            "requested_path": str(item),
            "snapshot_path": destination.relative_to(run_dir).as_posix(),
            "status": "selected_readable",
            "size": destination.stat().st_size,
            "sha256": _sha256(destination),
        })
    input_manifest = {
        "schema_version": "1.0",
        "source_packet": {
            "original_path": str(source),
            "snapshot_path": source_snapshot.relative_to(run_dir).as_posix(),
            "format": "structured_json" if source.suffix.lower() == ".json" else "markdown_compatibility",
            "size": source_snapshot.stat().st_size,
            "sha256": _sha256(source_snapshot),
        },
        "selected_files": input_rows,
        "missing_required_files": [],
        "optional_missing_files": [],
    }
    _write_json(run_dir / "input_manifest.json", input_manifest)
    envelope = {
        "contract_version": "1.0",
        "capability": "math-paper-writing",
        "operation": "write_paper",
        "writing_mode": writing_mode,
        "workflow_mode": normalized["workflow_mode"],
        "version": version,
        "language": request["language"],
        "executor": request["executor"],
        "run_dir": str(run_dir),
        "writer_dir": str(writer_dir),
        "task_output_dir": str(run_dir),
        "source_packet": str(source_snapshot),
        "input_manifest": str(run_dir / "input_manifest.json"),
        "selected_files": selected_snapshots,
        "instructions_file": str(Path(instructions_file).resolve()),
        "required_outputs": [
            "writer/article_candidate.tex",
            "writer/article_plan.md",
            "writer/claim_evidence_ledger.md",
            "writer/revision_notes.md"
        ],
        "constraints": {
            "authorized_root": str(run_dir),
            "all_outputs_inside_task_output_dir": True,
            "do_not_change_executor": True,
            "no_new_mathematical_claims": True,
            "no_fabricated_citations": True
        }
    }
    _write_json(run_dir / "execution_request.json", envelope)
    _event(run_dir, "prepared", executor=request["executor"])
    return envelope


def _collect_tex_sources(entry: Path, writer: Path) -> tuple[str, list[str]]:
    """Collect a local TeX source tree without following paths outside writer."""
    chunks: list[str] = []
    errors: list[str] = []
    visited: set[Path] = set()

    def visit(path: Path) -> None:
        resolved = path.resolve()
        try:
            resolved.relative_to(writer.resolve())
        except ValueError:
            errors.append(f"TeX include escapes writer directory: {path}")
            return
        if resolved in visited:
            return
        visited.add(resolved)
        if not resolved.is_file():
            errors.append(f"Missing dependency for TeX include: {path.relative_to(writer) if path.is_relative_to(writer) else path}")
            return
        text = resolved.read_text(encoding="utf-8", errors="replace")
        chunks.append(text)
        for raw in re.findall(r"\\(?:input|include)\s*\{([^}]+)\}", text):
            child = resolved.parent / raw.strip()
            if not child.suffix:
                child = child.with_suffix(".tex")
            visit(child)

    visit(entry)
    return "\n".join(chunks), errors


def _submission_metadata(text: str) -> dict[str, Any]:
    return {
        "author": bool(re.search(r"\\author\s*\{\s*[^}]", text)),
        "affiliation": bool(re.search(r"\\(?:affil|institute|address)\s*\{\s*[^}]", text)),
        "email": bool(re.search(r"\\(?:email|href\s*\{mailto:)", text)),
        "keywords": bool(re.search(r"\\keywords\s*\{|\\begin\s*\{keywords\}", text)),
        "msc": bool(re.search(r"(?:MSC|Mathematics Subject Classification)", text, re.IGNORECASE)),
    }


def _tex_gate(writer: Path) -> tuple[list[str], list[str], dict[str, Any]]:
    tex = writer / "article_candidate.tex"
    if not tex.is_file():
        return [], ["Missing writer/article_candidate.tex"], {}
    text, include_errors = _collect_tex_sources(tex, writer)
    warnings: list[str] = []
    errors: list[str] = list(include_errors)
    bibliographies = [writer / "refs.bib", writer / "references.bib"]
    cite_keys = set()
    for group in re.findall(r"\\cite\w*\s*(?:\[[^]]*\]\s*)*\{([^}]+)\}", text):
        cite_keys.update(key.strip() for key in group.split(",") if key.strip())
    if cite_keys:
        if not any(bib.is_file() for bib in bibliographies):
            errors.append("The paper contains citations but writer/refs.bib or writer/references.bib is missing")
        else:
            bib_keys = set(re.findall(r"@[A-Za-z]+\s*\{\s*([^,\s]+)", "\n".join(bib.read_text(encoding="utf-8", errors="replace") for bib in bibliographies if bib.is_file())))
            missing_keys = sorted(cite_keys - bib_keys)
            if missing_keys:
                errors.append("Citation keys missing from refs.bib: " + ", ".join(missing_keys))
    label_list = re.findall(r"\\label\{([^}]+)\}", text)
    labels = set(label_list)
    if len(labels) != len(label_list):
        errors.append("Duplicate TeX labels are present")
    refs = set(re.findall(r"\\(?:eqref|ref|autoref|cref|Cref)\{([^}]+)\}", text))
    missing_labels = sorted(refs - labels)
    if missing_labels:
        errors.append("References not defined in the collected TeX source tree: " + ", ".join(missing_labels))
    for command, raw in re.findall(r"\\(includegraphics)(?:\[[^]]*\])?\{([^}]+)\}", text):
        candidate = writer / raw
        if not candidate.resolve().is_relative_to(writer.resolve()):
            errors.append(f"Graphics dependency escapes writer directory: {raw}")
            continue
        candidates = [candidate]
        if not candidate.suffix:
            candidates.extend(candidate.with_suffix(ext) for ext in [".pdf", ".png", ".jpg", ".jpeg"])
        if not any(item.is_file() for item in candidates):
            errors.append(f"Missing dependency for \\{command}: {raw}")
    return warnings, errors, _submission_metadata(text)


def _compile(run_dir: Path, task: dict[str, Any]) -> tuple[str, list[str], list[str]]:
    writer = run_dir / "writer"
    tex = writer / "article_candidate.tex"
    if not tex.is_file():
        return "not_run", [], ["Missing writer/article_candidate.tex"]
    latexmk = shutil.which("latexmk")
    if not latexmk:
        return "not_run", ["latexmk is not available on PATH"], []
    engine = "-xelatex" if task["language"] in {"zh", "bilingual"} else "-pdf"
    # latexmk runs BibTeX from the output directory.  On Windows its MSYS
    # path conversion can corrupt non-ASCII workspace paths in BIBINPUTS,
    # even though XeLaTeX itself can read those paths.  Stage the declared
    # bibliography beside the .aux file so BibTeX resolves it by basename
    # without depending on an encoded absolute path.
    build_dir = writer / "build"
    _reset_check_directory(writer, build_dir)
    staged_bibliographies: list[Path] = []
    for bibliography in writer.glob("*.bib"):
        staged = build_dir / bibliography.name
        shutil.copy2(bibliography, staged)
        staged_bibliographies.append(staged)
    command = [latexmk, engine, "-no-shell-escape", "-interaction=nonstopmode", "-halt-on-error", "-file-line-error", "-outdir=build", tex.name]
    started = time.monotonic()
    try:
        completed = subprocess.run(command, cwd=writer, capture_output=True, text=True, errors="replace", timeout=task.get("time_limit_seconds", 3600))
        log = (completed.stdout or "") + "\n" + (completed.stderr or "")
        # MiKTeX's latexmk depends on an MSYS Perl process on Windows.  That
        # process can be denied permission even though the native TeX engine
        # is healthy.  Fall back to two direct engine passes so the caller
        # receives the real document error instead of a Perl infrastructure
        # error.  Bibliography-heavy documents still use latexmk whenever it
        # succeeds.
        direct_name = "xelatex" if engine == "-xelatex" else "pdflatex"
        direct_engine = shutil.which(direct_name)
        infrastructure_failure = bool(re.search(
            r"couldn't create signal pipe|Win32 error 5|perl(?:\.exe)?: fatal error|log4cxx:.*setFile",
            log,
            re.IGNORECASE,
        ))
        if (completed.returncode != 0 or not (build_dir / "article_candidate.pdf").is_file()) and direct_engine and infrastructure_failure:
            direct_command = [direct_engine, "-no-shell-escape", "-interaction=nonstopmode", "-halt-on-error", "-file-line-error", "-output-directory=build", tex.name]
            direct_logs = []
            for _ in range(2):
                completed = subprocess.run(direct_command, cwd=writer, capture_output=True, text=True, errors="replace", timeout=task.get("time_limit_seconds", 3600))
                direct_logs.append((completed.stdout or "") + "\n" + (completed.stderr or ""))
                if completed.returncode != 0:
                    break
            log += f"\n[mathcat] latexmk infrastructure failure; retried with {direct_name}.\n" + "\n".join(direct_logs)
            engine = f"-{direct_name}-direct"
    finally:
        for staged in staged_bibliographies:
            staged.unlink(missing_ok=True)
    elapsed = round(time.monotonic() - started, 3)
    (run_dir / "logs" / "latexmk.log").write_text(log, encoding="utf-8")
    pdf = writer / "build" / "article_candidate.pdf"
    if completed.returncode != 0 or not pdf.is_file():
        _write_json(run_dir / "logs" / "compile_report.json", {
            "status": "failed", "engine": engine.lstrip("-"), "duration_seconds": elapsed,
            "exit_code": completed.returncode, "log": "logs/latexmk.log"
        })
        tail = "\n".join(log.splitlines()[-30:])
        return "failed", [], ["LaTeX compilation failed. See logs/latexmk.log.\n" + tail]
    final_log_path = writer / "build" / "article_candidate.log"
    final_log = final_log_path.read_text(encoding="utf-8", errors="replace") if final_log_path.is_file() else log
    lower_log = final_log.lower()
    warnings = []
    if "there were undefined references" in lower_log or re.search(r"citation [`'][^\n]+ undefined", lower_log):
        warnings.append("Compilation reported undefined references or citations")
    overfull = []
    for match in re.finditer(r"Overfull \\hbox \((?P<amount>[0-9.]+)pt too wide\)(?:[^\n]*?at lines? (?P<lines>[0-9-]+))?", final_log):
        overfull.append({"amount_pt": float(match.group("amount")), "lines": match.group("lines")})
    if overfull:
        warnings.append(f"Compilation reported {len(overfull)} overfull hbox warning(s)")
    shutil.copy2(pdf, writer / "article_candidate.pdf")
    _write_json(run_dir / "logs" / "compile_report.json", {
        "status": "passed", "engine": engine.lstrip("-"), "duration_seconds": elapsed,
        "exit_code": completed.returncode, "log": "logs/latexmk.log", "overfull_hboxes": overfull,
        "undefined_references": any("undefined" in item.lower() for item in warnings)
    })
    # Keep one stable delivery PDF. The build copy is redundant and causes
    # platform artifact collectors to show the same paper twice.
    pdf.unlink(missing_ok=True)
    return "passed", warnings, []


def _render_pdf(run_dir: Path) -> tuple[str, dict[str, Any], list[str], list[str]]:
    writer = run_dir / "writer"
    pdf = writer / "article_candidate.pdf"
    report: dict[str, Any] = {"status": "not_run", "pdf": "writer/article_candidate.pdf", "page_count": 0, "rendered_pages": 0, "blank_pages": [], "inspection_level": "mechanical_render", "human_visual_review": "not_performed", "page_count_check": "not_run", "blank_page_check": "not_run"}
    if not pdf.is_file():
        return "not_run", report, [], ["PDF render check was not run because the delivery PDF is missing"]
    renderer = shutil.which("pdftoppm")
    if not renderer:
        return "not_run", report, ["pdftoppm is not available on PATH; visual rendering was not verified"], []
    started = time.monotonic()
    blank_pages: list[int] = []
    # Keep the render workspace under the authorized run directory.  On
    # Windows, Codex sandboxing can create system TemporaryDirectory folders
    # that the same subprocess cannot subsequently enumerate.
    pages_dir = run_dir / "logs" / "render-check"
    _reset_check_directory(run_dir, pages_dir)
    try:
        completed = subprocess.run([renderer, "-png", "-r", "120", str(pdf), str(pages_dir / "page")], capture_output=True, text=True, errors="replace", timeout=300)
        images = sorted(pages_dir.glob("page-*.png"))
        page_count = len(images)
        try:
            from pypdf import PdfReader
            page_count = len(PdfReader(str(pdf)).pages)
            report["page_count_check"] = "independent_pdf_reader"
        except Exception:
            pass
        try:
            from PIL import Image, ImageStat
            for index, image_path in enumerate(images, start=1):
                with Image.open(image_path) as image:
                    grayscale = image.convert("L").resize((160, 220))
                    stat = ImageStat.Stat(grayscale)
                    if stat.mean[0] > 254.7 and stat.var[0] < 1:
                        blank_pages.append(index)
            report["blank_page_check"] = "performed"
        except Exception:
            pass
        rendered_count = len(images)
    finally:
        pass  # Retain rendered pages inside the version for actual visual review.
    report.update({
        "status": "passed" if completed.returncode == 0 and rendered_count == page_count and page_count > 0 and not blank_pages else "failed",
        "renderer": renderer,
        "duration_seconds": round(time.monotonic() - started, 3),
        "page_count": page_count,
        "rendered_pages": rendered_count,
        "blank_pages": blank_pages,
        "retained_page_images": True,
        "page_images": [path.relative_to(run_dir).as_posix() for path in images],
    })
    errors = []
    if completed.returncode != 0:
        errors.append("PDF page rendering failed")
    if rendered_count != page_count or page_count == 0:
        errors.append(f"Rendered page count ({rendered_count}) does not match PDF page count ({page_count})")
    if blank_pages:
        errors.append("Potential blank PDF pages detected: " + ", ".join(map(str, blank_pages)))
    return report["status"], report, [], errors


def _artifact_type(path: Path) -> str | None:
    mapping = {
        "article_candidate.tex": "article_tex",
        "article_candidate.pdf": "article_pdf",
        "article_plan.md": "article_plan",
        "claim_evidence_ledger.md": "claim_evidence_ledger",
        "revision_notes.md": "revision_notes",
        "writing_report.md": "writing_report",
        "refs.bib": "bibliography",
        "references.bib": "bibliography",
        "human_revision_log.md": "human_revision_log",
        "style_decisions.md": "style_decisions",
        "sync_checklist.md": "sync_checklist",
        "sync_report.json": "sync_report",
        "manuscript_audit.json": "manuscript_audit",
        "related_work.tex": "related_work",
        "input_manifest.json": "input_manifest",
        "compile_report.json": "compile_report",
        "render_report.json": "render_report",
        "latexmk.log": "compile_log"
    }
    return mapping.get(path.name)


def _audit_manuscript(run_dir: Path) -> list[str]:
    """Use the bundled latest mechanical audit, without treating findings as proofs."""
    script = Path(__file__).resolve().parents[1] / "prompts" / "scripts" / "audit_manuscript.py"
    writer = run_dir / "writer"
    if not script.is_file() or not (writer / "article_candidate.tex").is_file():
        return []
    audit = runpy.run_path(str(script))
    keys = audit["parse_bib_keys"](list(writer.glob("*.bib")))
    findings, summaries = [], []
    for tex in sorted(writer.rglob("*.tex")):
        if "build" in tex.relative_to(writer).parts:
            continue
        rows, summary = audit["audit_tex"](tex, keys)
        findings.extend(rows)
        summaries.append(summary)
    revision = writer / "human_revision_log.md"
    if revision.is_file():
        findings.extend(audit["audit_revision_log"](revision))
    _write_json(run_dir / "logs" / "manuscript_audit.json", {
        "status": "review_candidates" if findings else "passed",
        "verification_level": "mechanical_lint_only", "summaries": summaries, "findings": findings,
    })
    return [f"Mechanical manuscript audit found {len(findings)} review candidate(s); see logs/manuscript_audit.json (not mathematical verification)"] if findings else []


def _discover(run_dir: Path) -> list[dict[str, Any]]:
    artifacts = []
    for path in sorted(run_dir.rglob("*")):
        if any(part in {"attempts", "primary_snapshot", "input_files"} for part in path.relative_to(run_dir).parts):
            continue
        if path == run_dir / "writer" / "build" / "article_candidate.pdf" and (run_dir / "writer" / "article_candidate.pdf").is_file():
            continue
        kind = _artifact_type(path)
        if not path.is_file() or not kind:
            continue
        digest = _sha256(path)
        artifacts.append({
            "type": kind,
            "path": path.relative_to(run_dir).as_posix(),
            "size": path.stat().st_size,
            "sha256": digest,
            "preview": "pdf" if path.suffix.lower() == ".pdf" else "text"
        })
    return artifacts


def finalize(run_dir_value: str | Path, *, publish_current: bool | None = None) -> dict[str, Any]:
    run_dir = Path(run_dir_value).resolve()
    task_file = run_dir / "task.json"
    if not task_file.is_file():
        raise CapabilityError(f"Not a prepared run directory: {run_dir}")
    task = json.loads(task_file.read_text(encoding="utf-8"))
    if (run_dir / "result.json").is_file():
        attempt = run_dir / "attempts" / datetime.now().strftime("%Y%m%d-%H%M%S-%f")
        attempt.mkdir(parents=True)
        for name in ("result.json", "artifacts.json", "writing_report.md", "logs/compile_report.json", "logs/render_report.json", "logs/latexmk.log"):
            source = run_dir / name
            if source.is_file():
                destination = attempt / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, destination)
    _event(run_dir, "finalizing")
    gate_warnings, gate_errors, submission_metadata = _tex_gate(run_dir / "writer")
    if not gate_errors:
        gate_warnings += _audit_manuscript(run_dir)
    delivery_checks_path = run_dir / "delivery_checks.json"
    if delivery_checks_path.is_file():
        delivery_checks = json.loads(delivery_checks_path.read_text(encoding="utf-8"))
        gate_errors += delivery_checks.get("errors", [])
        gate_warnings += delivery_checks.get("warnings", [])
    compile_status, warnings, errors = _compile(run_dir, task) if not gate_errors else ("not_run", [], [])
    if gate_errors:
        _write_json(run_dir / "logs" / "compile_report.json", {"status": "not_run", "reason": "source_gate_failed", "errors": gate_errors})
    render_status, render_report, render_warnings, render_errors = _render_pdf(run_dir) if compile_status == "passed" else ("not_run", {"status": "not_run"}, [], [])
    _write_json(run_dir / "logs" / "render_report.json", render_report)
    warnings = list(task.get("source_packet_warnings", [])) + gate_warnings + warnings
    warnings += render_warnings
    errors = gate_errors + errors + render_errors
    report_path = run_dir / "writing_report.md"
    preliminary = _discover(run_dir)
    types = {item["type"] for item in preliminary}
    required_before_report = {"article_tex", "article_plan", "claim_evidence_ledger", "revision_notes"}
    missing = sorted(required_before_report - types)
    if missing:
        errors.append("Missing required writing artifacts: " + ", ".join(missing))
    if compile_status == "not_run" and "article_tex" in types:
        warnings.append("TeX source exists, but PDF delivery was not verified")
    no_draft = "article_tex" not in types
    status = "completed" if not errors and compile_status == "passed" and render_status == "passed" and not any("undefined" in w.lower() for w in warnings) else ("failed" if no_draft else "partial")
    reason = "completed" if status == "completed" else ("compile_failed" if compile_status == "failed" else "delivery_gate_failed")
    input_manifest = json.loads((run_dir / "input_manifest.json").read_text(encoding="utf-8"))
    finished_at = datetime.now(timezone.utc).isoformat()
    related_work_status = "generated_in_paper_run" if (run_dir / "writer" / "related_work.tex").is_file() else "not_generated"
    report = [
        "# 论文写作任务报告", "",
        "## 交付摘要", "",
        f'- Writing mode: `{task.get("writing_mode", "new_draft")}`',
        f'- Primary PDF: `writer/article_candidate.pdf`',
        f'- Primary TeX: `writer/article_candidate.tex`',
        f'- Status: `{status}`',
        f'- Compile: `{compile_status}`; render: `{render_status}`',
        f'- Warning count: `{len(warnings)}`; error count: `{len(errors)}`', "",
        "## 1. 任务信息", "",
        f'- Task: `{task["task_id"]}`',
        f'- Executor: `{task["executor"]}`',
        f'- Version: `{task["version"]}`',
        f'- Started: `{task.get("created_at", "unknown")}`',
        f'- Finished: `{finished_at}`',
        f'- Status: `{status}`',
        f'- Termination reason: `{reason}`',
        f'- Compile status: `{compile_status}`',
        f'- Render status: `{render_status}`',
        "- Verification level: `writing_audit`（不承担数学或形式化验证）", "",
        "## 2. 输入材料", "",
        f'- Source Packet: `{input_manifest["source_packet"]["snapshot_path"]}`',
        f'- Source Packet format: `{input_manifest["source_packet"]["format"]}`',
        f'- Selected files: `{len(input_manifest["selected_files"])}`',
        "- Missing required files: `0`", "",
    ]
    report.extend([f'- `{row["requested_path"]}` → `{row["snapshot_path"]}`' for row in input_manifest["selected_files"]] or ["- 未另外选择输入文件"])
    report.extend(["", "## 3. 执行过程摘要", "",
                   f"- 已检查材料和写作产物；本次编译状态：{compile_status}；本次机械渲染状态：{render_status}。未执行的步骤不计作完成。",
                   f"- Related work status: `{related_work_status}`。当前未安装独立 `math-related-work` 能力时，不声称发生跨能力调用。", "",
                   "## 4. 可信度边界", "",
                   "- 本报告记录文件门禁、编译和机械渲染状态；不等同于来源逐句人工核验或人工版式审阅。",
                   "- 数学结论的人工审阅、形式化验证和新颖性判断属于其他能力或人工流程。", "",
                   "## 5. 编译与渲染", "",
                   f'- PDF pages: `{render_report.get("page_count", 0)}`',
                   f'- Rendered pages: `{render_report.get("rendered_pages", 0)}`',
                   f'- Potential blank pages: `{render_report.get("blank_pages", [])}`', "",
                   "## 6. 投稿元数据审计", "",
                   f'- Author: `{submission_metadata.get("author", False)}`',
                   f'- Affiliation: `{submission_metadata.get("affiliation", False)}`',
                   f'- Email: `{submission_metadata.get("email", False)}`',
                   f'- Keywords: `{submission_metadata.get("keywords", False)}`',
                   f'- MSC: `{submission_metadata.get("msc", False)}`',
                   "- 缺失项不会被自动编造；目标期刊未指定时不阻断论文写作交付。", "",
                   "## 7. Warnings", ""])
    report.extend([f"- {item}" for item in warnings] or ["- None"])
    report.extend(["", "## 8. Errors", ""])
    report.extend([f"- {item}" for item in errors] or ["- None"])
    report.extend(["", "## 9. 成果清单", ""])
    report.extend([f'- `{item["type"]}`: `{item["path"]}`' for item in preliminary] or ["- None"])
    report.extend(["", "## 10. 建议下一步", "", "- 检查版式警告与投稿元数据缺口。", "- 需要数学确认时，将论文交给相应研究/验证板块。"])
    report_path.write_text("\n".join(report) + "\n", encoding="utf-8")
    artifacts = _discover(run_dir)
    _write_json(run_dir / "artifacts.json", {"schema_version": "1.0", "artifacts": artifacts})
    result = {
        "schema_version": "1.0",
        "task_id": task["task_id"],
        "executor": task["executor"],
        "status": status,
        "termination_reason": reason,
        "version": task["version"],
        "compile_status": compile_status,
        "render_status": render_status,
        "inspection_level": "mechanical_render",
        "human_visual_review": "not_performed",
        "verification_level": "writing_audit",
        "submission_metadata": submission_metadata,
        "artifacts": artifacts,
        "warnings": warnings,
        "errors": errors
    }
    _write_json(run_dir / "result.json", result)
    _event(run_dir, "finished", status=status, termination_reason=reason)
    if status == "completed" and (task.get("publish_current", True) if publish_current is None else publish_current):
        current_dir = Path(task["workspace"]) / "论文" / "current"
        current_dir.mkdir(parents=True, exist_ok=True)
        primary_pdf = run_dir / "writer" / "article_candidate.pdf"
        _write_json(current_dir / "current.json", {
            "version": task["version"], "run_dir": str(run_dir),
            "primary_tex": str(run_dir / "writer" / "article_candidate.tex"),
            "primary_pdf": str(primary_pdf),
            "writing_report": str(report_path)
        })
        (current_dir / "README.md").write_text(
            "# 当前论文版本\n\n"
            f"- Version: `{task['version']}`\n"
            f"- PDF: `{primary_pdf}`\n"
            f"- TeX: `{run_dir / 'writer' / 'article_candidate.tex'}`\n"
            f"- Report: `{report_path}`\n",
            encoding="utf-8"
        )
    return result


def terminate(run_dir_value: str | Path, reason: str, message: str = "") -> dict[str, Any]:
    if reason not in TERMINATION_REASONS:
        raise CapabilityError("Termination reason must be executor_failed, user_stopped, or time_limit")
    run_dir = Path(run_dir_value).resolve()
    task_file = run_dir / "task.json"
    if not task_file.is_file():
        raise CapabilityError(f"Not a prepared run directory: {run_dir}")
    task = json.loads(task_file.read_text(encoding="utf-8"))
    artifacts = _discover(run_dir)
    status = "stopped" if reason == "user_stopped" else ("partial" if artifacts else "failed")
    errors = [message] if message else []
    report_path = run_dir / "writing_report.md"
    report_path.write_text(
        "# Paper Writing Report\n\n"
        f'- Task: `{task["task_id"]}`\n'
        f'- Executor: `{task["executor"]}`\n'
        f'- Version: `{task["version"]}`\n'
        f'- Status: `{status}`\n'
        f'- Termination reason: `{reason}`\n'
        "- Compile status: `not_run`\n"
        "- Verification level: `writing_audit`\n\n"
        + (f"## Diagnostic\n\n{message}\n" if message else ""),
        encoding="utf-8"
    )
    artifacts = _discover(run_dir)
    _write_json(run_dir / "artifacts.json", {"schema_version": "1.0", "artifacts": artifacts})
    result = {
        "schema_version": "1.0", "task_id": task["task_id"], "executor": task["executor"],
        "status": status, "termination_reason": reason, "version": task["version"],
        "compile_status": "not_run", "verification_level": "writing_audit",
        "artifacts": artifacts, "warnings": [], "errors": errors
    }
    _write_json(run_dir / "result.json", result)
    _event(run_dir, "finished", status=status, termination_reason=reason)
    return result
