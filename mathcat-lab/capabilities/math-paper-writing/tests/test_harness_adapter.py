import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


PACKAGE_ROOT = Path(__file__).resolve().parents[1]
ADAPTER = PACKAGE_ROOT / "adapters" / "deepseek_harness" / "adapter.py"


class HarnessAdapterTests(unittest.TestCase):
    def call(self, *args: str, cwd: str | None = None) -> subprocess.CompletedProcess[str]:
        return subprocess.run([sys.executable, str(ADAPTER), *args], cwd=cwd, capture_output=True, text=True, encoding="utf-8")

    def test_health_is_one_json_document_and_cwd_independent(self):
        with tempfile.TemporaryDirectory() as temp:
            completed = self.call("health", cwd=temp)
        self.assertEqual(completed.returncode, 0)
        payload = json.loads(completed.stdout)
        self.assertTrue(payload["ok"])
        self.assertTrue(payload["data"]["ready"])
        self.assertEqual(completed.stderr, "")

    def test_prepare_returns_stable_envelope(self):
        with tempfile.TemporaryDirectory() as temp:
            workspace = Path(temp)
            (workspace / "source.md").write_text("# Source Packet", encoding="utf-8")
            request = workspace / "request.json"
            request.write_text(json.dumps({
                "schema_version": "1.0", "task_id": "host", "executor": "deepseek_harness",
                "workspace": str(workspace), "source_packet": "source.md", "language": "zh"
            }), encoding="utf-8")
            completed = self.call("prepare", "--request", str(request), cwd=str(workspace))
            payload = json.loads(completed.stdout)
            self.assertEqual(completed.returncode, 0)
            self.assertTrue(payload["ok"])
            self.assertEqual(payload["operation"], "prepare")
            self.assertEqual(payload["data"]["executor"], "deepseek_harness")

    def test_error_is_machine_readable(self):
        completed = self.call("prepare", "--request", "does-not-exist.json")
        self.assertEqual(completed.returncode, 2)
        payload = json.loads(completed.stdout)
        self.assertFalse(payload["ok"])
        self.assertEqual(payload["error"]["code"], "CAPABILITY_ERROR")


if __name__ == "__main__":
    unittest.main()
