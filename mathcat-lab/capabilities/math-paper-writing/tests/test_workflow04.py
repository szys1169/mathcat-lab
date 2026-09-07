import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from math_paper_writing.core import CapabilityError, _reset_check_directory, _tex_gate, finalize, health, prepare

ROOT = Path(__file__).resolve().parents[1]


class Workflow04Tests(unittest.TestCase):
    def request(self, workspace, **extra):
        (workspace / "source.md").write_text("Exact supplied source", encoding="utf-8")
        request = {"schema_version": "1.0", "task_id": "stable", "executor": "codex", "workspace": str(workspace), "source_packet": "source.md", "language": "zh", "idempotency_key": "stable", **extra}
        file = workspace / "request.json"
        file.write_text(json.dumps(request), encoding="utf-8")
        return file

    def test_latest_workflow_bundle_matches_all_provenance_hashes(self):
        manifest = json.loads((ROOT / "workflow-source.json").read_text(encoding="utf-8"))
        self.assertFalse(manifest["source_has_semantic_version"])
        self.assertEqual(health(ROOT)["capability_version"], "0.4.0")
        for file in manifest["files"]:
            self.assertEqual(hashlib.sha256((ROOT / file["bundled"]).read_bytes()).hexdigest(), file["sha256"], file["source"])
        names = {file["source"] for file in manifest["files"]}
        self.assertIn("references/human-revision-and-bilingual-sync.md", names)
        self.assertIn("scripts/audit_manuscript.py", names)

    def test_prepare_retry_recovers_one_version_and_rejects_changed_request(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            request = self.request(workspace)
            first = prepare(request, ROOT / "prompts/writing.md")
            (Path(first["writer_dir"]) / "article_candidate.tex").write_text("keep this draft", encoding="utf-8")
            again = prepare(request, ROOT / "prompts/writing.md")
            self.assertEqual(first, again)
            self.assertEqual(len(list((workspace / "论文/versions").iterdir())), 1)
            self.assertEqual((Path(again["writer_dir"]) / "article_candidate.tex").read_text(), "keep this draft")
            request = self.request(workspace, language="en")
            with self.assertRaisesRegex(CapabilityError, "different paper inputs"):
                prepare(request, ROOT / "prompts/writing.md")

    def test_prepare_recovers_interrupted_snapshot_without_second_version(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            request = self.request(workspace)
            first = prepare(request, ROOT / "prompts/writing.md")
            (Path(first["run_dir"]) / "execution_request.json").unlink()
            again = prepare(request, ROOT / "prompts/writing.md")
            self.assertEqual(first, again)
            self.assertEqual(len(list((workspace / "论文/versions").iterdir())), 1)

    def test_latest_bibliography_name_and_duplicate_label_gate(self):
        with tempfile.TemporaryDirectory() as temp:
            writer = Path(temp)
            (writer / "article_candidate.tex").write_text(r"\cite{x}\label{one}", encoding="utf-8")
            (writer / "references.bib").write_text("@article{x,title={X}}", encoding="utf-8")
            self.assertEqual(_tex_gate(writer)[1], [])
            (writer / "article_candidate.tex").write_text(r"\label{one}\label{one}", encoding="utf-8")
            self.assertIn("Duplicate TeX labels are present", _tex_gate(writer)[1])

    def test_check_directory_reset_is_limited_to_owned_child(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            writer = root / "writer"
            build = writer / "build"
            build.mkdir(parents=True)
            (build / "stale.aux").write_text("old", encoding="utf-8")
            _reset_check_directory(writer, build)
            self.assertEqual(list(build.iterdir()), [])
            outside = root / "outside"
            outside.mkdir()
            (outside / "keep.md").write_text("keep", encoding="utf-8")
            with self.assertRaisesRegex(CapabilityError, "escapes"):
                _reset_check_directory(writer, outside)
            self.assertTrue((outside / "keep.md").is_file())

    def test_no_publication_and_retry_preserves_failed_receipt(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            prepared = prepare(self.request(workspace, publish_current=False), ROOT / "prompts/writing.md")
            failed = finalize(prepared["run_dir"])
            self.assertEqual(failed["status"], "failed")
            writer = Path(prepared["writer_dir"])
            (writer / "article_candidate.tex").write_text(r"\documentclass{article}\begin{document}Supplied fact.\end{document}", encoding="utf-8")
            (writer / "article_candidate.pdf").write_bytes(b"%PDF-1.4 fixture")
            for name in ("article_plan.md", "claim_evidence_ledger.md", "revision_notes.md"):
                (writer / name).write_text("fixture", encoding="utf-8")
            with patch("math_paper_writing.core._compile", return_value=("passed", [], [])), patch("math_paper_writing.core._render_pdf", return_value=("passed", {"page_count": 1}, [], [])):
                passed = finalize(prepared["run_dir"])
            self.assertEqual(passed["status"], "completed")
            self.assertFalse((workspace / "论文/current/current.json").exists())
            old = list((Path(prepared["run_dir"]) / "attempts").glob("*/result.json"))
            self.assertEqual(len(old), 1)
            self.assertEqual(json.loads(old[0].read_text())["status"], "failed")

    def test_delivery_checks_block_compile_and_never_claim_it_ran(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            prepared = prepare(self.request(workspace), ROOT / "prompts/writing.md")
            run = Path(prepared["run_dir"])
            (run / "writer/article_candidate.tex").write_text("draft", encoding="utf-8")
            (run / "delivery_checks.json").write_text(json.dumps({"errors": ["Changed primary manuscript"]}), encoding="utf-8")
            with patch("math_paper_writing.core._compile") as compile_mock:
                result = finalize(run)
                compile_mock.assert_not_called()
            self.assertEqual(result["compile_status"], "not_run")
            self.assertEqual(result["status"], "partial")
            self.assertIn("Changed primary manuscript", result["errors"])
            self.assertFalse((workspace / "论文/current/current.json").exists())


if __name__ == "__main__":
    unittest.main()
