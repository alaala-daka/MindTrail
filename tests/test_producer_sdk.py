import json
import math
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "sdk" / "python"))
from mindtrail_producer import MindTrailProducer

ROOT = pathlib.Path(__file__).resolve().parents[1]
EVENT = {"v": 1, "session": "s", "seq": 1, "ts": "2024-01-01T00:00:00Z", "type": "task.start", "payload": {"title": "标题", "goal": "G", "project": "../path"}}


class ProducerSdkTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        directory = pathlib.Path(tempfile.mkdtemp(prefix="sdk fixture ", dir=ROOT / ".tools"))
        cls.executable = directory / "fake producer.exe"
        subprocess.run([os.environ.get("MINDTRAIL_RUSTC", "rustc"), str(ROOT / "tests/fixtures/producer_fixture.rs"), "-o", str(cls.executable)], check=True, timeout=60)

    def test_exact_utf8_and_metacharacter_argument(self):
        batch = "id & echo DO_NOT_RUN | x"
        os.environ["MINDTRAIL_EXPECT_STDIN"] = json.dumps([EVENT], ensure_ascii=False, separators=(",", ":"))
        try:
            self.assertEqual(MindTrailProducer(str(self.executable)).submit_events("q", batch, [EVENT]), {"status": "queued", "batch_id": batch, "session": "s"})
        finally:
            del os.environ["MINDTRAIL_EXPECT_STDIN"]

    def test_errors_and_flush(self):
        sdk = MindTrailProducer(str(self.executable))
        self.assertEqual(sdk.submit_events("q", "test-error", [EVENT]), {"error": {"code": "seq_gap", "message": "missing", "expected_seq": 2, "offending_seq": 3, "last_seq": 1, "index": 0}})
        self.assertEqual(sdk.flush("q"), {"committed": 1, "pending": 2, "blocked": 3})
        self.assertEqual(sdk.submit_events("q", "valid-committed", [EVENT]), {"status": "committed", "batch_id": "valid-committed", "session": "s", "receipt": {"accepted": 1, "duplicates": 0, "last_seq": 1}})

    def test_bad_children(self):
        sdk = MindTrailProducer(str(self.executable), max_output_bytes=2048)
        for batch in ("bad-json", "duplicate", "bad-shape", "wrong-id", "wrong-session", "wrong-count", "low-watermark", "error-zero", "error-two", "invalid-utf8", "exit", "flood", "stderr-flood"):
            with self.subTest(batch=batch):
                self.assertEqual(sdk.submit_events("q", batch, [EVENT])["error"]["code"], "producer_failed")
        self.assertEqual(MindTrailProducer(str(ROOT / "missing-producer.exe")).flush("q")["error"]["code"], "producer_failed")
        self.assertEqual(MindTrailProducer("relative.exe").flush("q")["error"]["code"], "producer_failed")

    def test_timeout_and_nonfinite(self):
        sdk = MindTrailProducer(str(self.executable), timeout=.15)
        marker = str(self.executable) + ".survived"
        os.environ["MINDTRAIL_FIXTURE_SURVIVAL_MARKER"] = marker
        try:
            self.assertEqual(sdk.submit_events("q", "hang", [EVENT])["error"]["code"], "producer_failed")
            time.sleep(1)
            self.assertFalse(pathlib.Path(marker).exists())
        finally:
            del os.environ["MINDTRAIL_FIXTURE_SURVIVAL_MARKER"]
        for value in (math.nan, math.inf, -math.inf):
            event = {**EVENT, "payload": {"value": value}}
            self.assertEqual(sdk.submit_events("q", "x", [event])["error"]["code"], "invalid_request")


if __name__ == "__main__":
    unittest.main()
