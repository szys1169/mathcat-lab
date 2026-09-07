import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from math_paper_writing.core import CapabilityError, _compile, _tex_gate, finalize, preflight, prepare, terminate


class CoreTests(unittest.TestCase):
    def test_tex_gate_resolves_labels_and_citations_across_inputs(self):
        with tempfile.TemporaryDirectory() as temp:
            writer = Path(temp)
            (writer / "article_candidate.tex").write_text(
                r"\documentclass{article}\begin{document}\input{related_work}\ref{sec:background}\end{document}",
                encoding="utf-8",
            )
            (writer / "related_work.tex").write_text(r"\section{Background}\label{sec:background}", encoding="utf-8")
            warnings, errors, metadata = _tex_gate(writer)
            self.assertEqual(warnings, [])
            self.assertEqual(errors, [])
            self.assertFalse(metadata["affiliation"])

    def test_tex_gate_rejects_include_outside_writer(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            writer = root / "writer"
            writer.mkdir()
            (root / "outside.tex").write_text("outside", encoding="utf-8")
            (writer / "article_candidate.tex").write_text(r"\input{../outside}", encoding="utf-8")
            _, errors, _ = _tex_gate(writer)
            self.assertTrue(any("escapes writer" in error for error in errors))

    def test_preflight_has_no_version_side_effect_and_classifies_existing_manuscript(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            (workspace / "source.md").write_text("# Source Packet", encoding="utf-8")
            (workspace / "article_candidate.tex").write_text("draft", encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "preflight", "executor": "codex",
                "workspace": str(workspace), "source_packet": "source.md", "language": "en",
                "selected_files": ["article_candidate.tex"]
            }), encoding="utf-8")
            result = preflight(request)
            self.assertTrue(result["ready"])
            self.assertEqual(result["writing_mode"], "assemble_existing")
            self.assertFalse((workspace / "论文" / "versions").exists())

    def test_prepare_rejects_source_outside_workspace(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            workspace = root / "workspace"
            workspace.mkdir()
            outside = root / "outside.md"
            outside.write_text("source", encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "test", "executor": "codex",
                "workspace": str(workspace), "source_packet": str(outside), "language": "en"
            }), encoding="utf-8")
            with self.assertRaises(CapabilityError):
                prepare(request, Path(__file__))

    def test_prepare_and_partial_finalize_without_tex(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            source = workspace / "source.md"
            source.write_text("# Source Packet", encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "test", "executor": "deepseek_harness",
                "workspace": str(workspace), "source_packet": "source.md", "language": "en"
            }), encoding="utf-8")
            envelope = prepare(request, Path(__file__))
            result = finalize(envelope["run_dir"])
            self.assertEqual(result["status"], "failed")
            self.assertTrue((Path(envelope["run_dir"]) / "result.json").is_file())

    def test_selected_files_are_snapshotted(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            source = workspace / "source.md"
            source.write_text("# Source Packet", encoding="utf-8")
            bibliography = workspace / "refs.bib"
            bibliography.write_text("@book{x,title={X}}", encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "snapshot", "executor": "deepseek_harness",
                "workspace": str(workspace), "source_packet": "source.md", "language": "en",
                "selected_files": ["refs.bib"]
            }), encoding="utf-8")
            envelope = prepare(request, Path(__file__))
            self.assertEqual(len(envelope["selected_files"]), 1)
            self.assertTrue(Path(envelope["selected_files"][0]).is_file())
            manifest = json.loads((Path(envelope["run_dir"]) / "input_manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["selected_files"][0]["status"], "selected_readable")
            self.assertEqual(manifest["missing_required_files"], [])

    def test_prepare_keeps_structured_source_packet_extension(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            source = workspace / "source.json"
            source.write_text(json.dumps({
                "schema_version": "1.0",
                "writing_goal": {"deliverable": "paper", "audience": "researchers", "language": "en"},
                "core_results": [{"id": "R1", "statement": "S", "status": "proved", "sources": ["note.md"]}],
                "source_materials": [], "citation_whitelist": [], "evidence_gaps": []
            }), encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "json-source", "executor": "codex",
                "workspace": str(workspace), "source_packet": "source.json", "language": "en"
            }), encoding="utf-8")
            envelope = prepare(request, Path(__file__))
            self.assertTrue(envelope["source_packet"].endswith("source_packet.json"))
            self.assertTrue(Path(envelope["source_packet"]).is_file())

    def test_finalize_detects_missing_bibliography_key(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            (workspace / "source.md").write_text("# Source Packet", encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "citations", "executor": "codex",
                "workspace": str(workspace), "source_packet": "source.md", "language": "en"
            }), encoding="utf-8")
            envelope = prepare(request, Path(__file__))
            writer = Path(envelope["writer_dir"])
            (writer / "article_candidate.tex").write_text(r"\documentclass{article}\begin{document}\cite{missing}\end{document}", encoding="utf-8")
            for name in ("article_plan.md", "claim_evidence_ledger.md", "revision_notes.md"):
                (writer / name).write_text("ok", encoding="utf-8")
            result = finalize(envelope["run_dir"])
            self.assertEqual(result["status"], "partial")
            self.assertTrue(any("refs.bib" in error for error in result["errors"]))

    def test_terminate_records_user_stop_without_current(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            (workspace / "source.md").write_text("# Source Packet", encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "stop", "executor": "deepseek_harness",
                "workspace": str(workspace), "source_packet": "source.md", "language": "zh"
            }), encoding="utf-8")
            envelope = prepare(request, Path(__file__))
            result = terminate(envelope["run_dir"], "user_stopped", "Stopped by user")
            self.assertEqual(result["status"], "stopped")
            self.assertFalse((workspace / "论文" / "current" / "current.json").exists())

    def test_compile_rebuilds_clean_directory_and_stages_bibliography(self):
        with tempfile.TemporaryDirectory() as temp:
            run_dir = Path(temp) / "中文路径" / "run"
            writer = run_dir / "writer"
            logs = run_dir / "logs"
            build = writer / "build"
            build.mkdir(parents=True)
            logs.mkdir()
            (writer / "article_candidate.tex").write_text(
                r"\documentclass{article}\begin{document}\cite{sample}\bibliography{refs}\end{document}",
                encoding="utf-8",
            )
            (writer / "refs.bib").write_text("@article{sample,title={Sample}}", encoding="utf-8")
            (build / "stale.aux").write_text("stale", encoding="utf-8")

            def fake_run(command, **kwargs):
                self.assertEqual(Path(kwargs["cwd"]), writer)
                self.assertFalse((build / "stale.aux").exists())
                self.assertTrue((build / "refs.bib").is_file())
                (build / "article_candidate.pdf").write_bytes(b"%PDF-1.4\n")
                (build / "article_candidate.log").write_text("clean compile", encoding="utf-8")
                return subprocess.CompletedProcess(command, 0, stdout="ok", stderr="")

            with patch("math_paper_writing.core.shutil.which", return_value="latexmk"), patch(
                "math_paper_writing.core.subprocess.run", side_effect=fake_run
            ):
                status, warnings, errors = _compile(run_dir, {"language": "en", "time_limit_seconds": 60})

            self.assertEqual(status, "passed")
            self.assertEqual(warnings, [])
            self.assertEqual(errors, [])
            self.assertFalse((build / "refs.bib").exists())
            self.assertFalse((build / "article_candidate.pdf").exists())
            self.assertTrue((writer / "article_candidate.pdf").read_bytes().startswith(b"%PDF-"))

    def test_failed_compile_removes_staged_bibliography_and_reports_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            run_dir = Path(temp) / "run"
            writer = run_dir / "writer"
            logs = run_dir / "logs"
            writer.mkdir(parents=True)
            logs.mkdir()
            (writer / "article_candidate.tex").write_text("broken", encoding="utf-8")
            (writer / "refs.bib").write_text("@article{x,title={X}}", encoding="utf-8")

            def fake_run(command, **kwargs):
                self.assertTrue((writer / "build" / "refs.bib").is_file())
                return subprocess.CompletedProcess(command, 12, stdout="compile failed", stderr="bibtex failed")

            with patch("math_paper_writing.core.shutil.which", return_value="latexmk"), patch(
                "math_paper_writing.core.subprocess.run", side_effect=fake_run
            ):
                status, warnings, errors = _compile(run_dir, {"language": "en", "time_limit_seconds": 60})

            self.assertEqual(status, "failed")
            self.assertEqual(warnings, [])
            self.assertTrue(any("LaTeX compilation failed" in error for error in errors))
            self.assertFalse((writer / "build" / "refs.bib").exists())
            report = json.loads((logs / "compile_report.json").read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "failed")

    def test_compile_falls_back_to_native_engine_when_latexmk_perl_is_blocked(self):
        with tempfile.TemporaryDirectory() as temp:
            run_dir = Path(temp) / "run"
            writer = run_dir / "writer"
            build = writer / "build"
            writer.mkdir(parents=True)
            (run_dir / "logs").mkdir()
            (writer / "article_candidate.tex").write_text(
                r"\documentclass{article}\begin{document}ok\end{document}", encoding="utf-8"
            )
            calls = []

            def fake_which(name):
                return {"latexmk": "latexmk", "pdflatex": "pdflatex"}.get(name)

            def fake_run(command, **kwargs):
                calls.append(command[0])
                if command[0] == "latexmk":
                    return subprocess.CompletedProcess(command, 1, stdout="", stderr="perl.exe: fatal error - couldn't create signal pipe, Win32 error 5")
                (build / "article_candidate.pdf").write_bytes(b"%PDF-1.4\n")
                (build / "article_candidate.log").write_text("clean compile", encoding="utf-8")
                return subprocess.CompletedProcess(command, 0, stdout="native ok", stderr="")

            with patch("math_paper_writing.core.shutil.which", side_effect=fake_which), patch(
                "math_paper_writing.core.subprocess.run", side_effect=fake_run
            ):
                status, warnings, errors = _compile(run_dir, {"language": "en", "time_limit_seconds": 60})

            self.assertEqual(status, "passed")
            self.assertEqual(warnings, [])
            self.assertEqual(errors, [])
            self.assertEqual(calls, ["latexmk", "pdflatex", "pdflatex"])
            report = json.loads((run_dir / "logs" / "compile_report.json").read_text(encoding="utf-8"))
            self.assertEqual(report["engine"], "pdflatex-direct")

    @unittest.skipUnless(shutil.which("latexmk") and shutil.which("pdftoppm"), "LaTeX and Poppler are required")
    def test_real_compile_render_and_current_delivery(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            (workspace / "source.md").write_text("# Source Packet", encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "real-delivery", "executor": "codex",
                "workspace": str(workspace), "source_packet": "source.md", "language": "en"
            }), encoding="utf-8")
            envelope = prepare(request, Path(__file__))
            writer = Path(envelope["writer_dir"])
            (writer / "article_candidate.tex").write_text(
                "\\documentclass{article}\\begin{document}Verified delivery.\\end{document}",
                encoding="utf-8"
            )
            for name in ("article_plan.md", "claim_evidence_ledger.md", "revision_notes.md"):
                (writer / name).write_text("ok", encoding="utf-8")
            result = finalize(envelope["run_dir"])
            self.assertEqual(result["status"], "completed")
            self.assertEqual(result["render_status"], "passed")
            self.assertTrue((writer / "article_candidate.pdf").is_file())
            self.assertFalse((writer / "build" / "article_candidate.pdf").exists())
            self.assertFalse((writer / "rendered_pages").exists())
            current = json.loads((workspace / "论文" / "current" / "current.json").read_text(encoding="utf-8"))
            self.assertEqual(current["primary_pdf"], str(writer / "article_candidate.pdf"))


if __name__ == "__main__":
    unittest.main()
