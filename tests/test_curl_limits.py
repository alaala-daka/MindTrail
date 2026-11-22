import json
import os
import pathlib
import socket
import subprocess
import tempfile
import threading
import time
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
TOKEN = "a" * 64
STORE = "b" * 64
EVENT = [{"v": 1, "session": "curl-test", "seq": 1, "ts": "2024-01-01T00:00:00Z", "type": "task.start", "payload": {"title": "T", "goal": "G", "project": "P"}}]


class CurlSafetyTests(unittest.TestCase):
    def setUp(self):
        self.base = pathlib.Path(tempfile.mkdtemp(prefix="curl safety ", dir=ROOT / ".tools"))
        self.discovery = self.base / "endpoint.json"
        self.batch = self.base / "batch.json"
        self.batch.write_text(json.dumps(EVENT), encoding="utf-8")

    def run_script(self):
        pwsh = os.environ.get("MINDTRAIL_TEST_PWSH", "pwsh")
        result = subprocess.run([pwsh, "-NoProfile", "-File", str(ROOT / "sdk/examples/submit-events.ps1"),
                                 "-DiscoveryPath", str(self.discovery), "-BatchPath", str(self.batch),
                                 "-ScratchDirectory", str(self.base)], capture_output=True, timeout=15,
                                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        self.assertNotIn(TOKEN.encode(), result.stdout)
        self.assertNotIn(TOKEN.encode(), result.stderr)
        return result

    def serve(self, responses):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(8)
        self.discovery.write_text(json.dumps({"version": 1, "port": listener.getsockname()[1], "token": TOKEN, "store_id": STORE}), encoding="utf-8")
        paths = []
        peak = [0]
        def worker():
            try:
                for body, unknown_length in responses:
                    conn, _ = listener.accept()
                    with conn:
                        conn.settimeout(3)
                        request = b""
                        while b"\r\n\r\n" not in request:
                            request += conn.recv(4096)
                        header, rest = request.split(b"\r\n\r\n", 1)
                        paths.append(header.split(b"\r\n", 1)[0].decode())
                        length = next((int(line.split(b":", 1)[1]) for line in header.split(b"\r\n") if line.lower().startswith(b"content-length:")), 0)
                        while len(rest) < length:
                            rest += conn.recv(4096)
                        headers = b"HTTP/1.1 200 OK\r\nConnection: close\r\n" + (b"" if unknown_length else f"Content-Length: {len(body)}\r\n".encode()) + b"\r\n"
                        try:
                            conn.sendall(headers)
                            for offset in range(0, len(body), 4096):
                                conn.sendall(body[offset:offset+4096])
                                for file in self.base.iterdir():
                                    if file not in (self.discovery, self.batch):
                                        try: peak[0] = max(peak[0], file.stat().st_size)
                                        except FileNotFoundError: pass
                                time.sleep(.001)
                        except (OSError, ConnectionError):
                            pass
            finally:
                listener.close()
        thread = threading.Thread(target=worker, daemon=True)
        thread.start()
        return thread, paths, peak

    def test_discovery_size_and_duplicate_keys(self):
        for content in (" " * 4097, '{"version":1,"version":1,"port":123,"token":"' + TOKEN + '","store_id":"' + STORE + '"}'):
            self.discovery.write_text(content, encoding="utf-8")
            self.assertNotEqual(self.run_script().returncode, 0)

    def test_health_rejects_oversized_unknown_length_and_bad_shapes(self):
        bodies = [b"x" * 250_000,
                  ('{"version":1,"\\u0076ersion":1,"store_id":"' + STORE + '"}').encode(),
                  ('{"version":"1","store_id":"' + STORE + '"}').encode()]
        for body in bodies:
            with self.subTest(length=len(body)):
                thread, paths, peak = self.serve([(body, True)])
                self.assertNotEqual(self.run_script().returncode, 0)
                thread.join(timeout=5)
                self.assertFalse(thread.is_alive())
                self.assertEqual(paths, ["GET /v1/health HTTP/1.1"])
                self.assertLessEqual(peak[0], 65_536)

    def test_receipt_size_and_watermark_are_enforced(self):
        health = json.dumps({"version": 1, "store_id": STORE}).encode()
        for body in (b"x" * 250_000, b'{"accepted":1,"duplicates":0,"last_seq":0}'):
            with self.subTest(length=len(body)):
                thread, paths, peak = self.serve([(health, False), (body, True)])
                self.assertNotEqual(self.run_script().returncode, 0)
                thread.join(timeout=5)
                self.assertFalse(thread.is_alive())
                self.assertEqual(paths, ["GET /v1/health HTTP/1.1", "POST /v1/events HTTP/1.1"])
                self.assertLessEqual(peak[0], 65_536)


if __name__ == "__main__":
    unittest.main()
