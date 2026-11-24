from __future__ import annotations

import os
import tempfile
import threading
import unittest
from pathlib import Path

from guanhai_bafen import BafenCore, BafenError


class PythonAdapterTests(unittest.TestCase):
    def setUp(self):
        configured = os.environ.get("GUANHAI_CORE")
        if not configured or not Path(configured).is_file():
            self.skipTest("Set GUANHAI_CORE to the compiled core executable")
        self.core = BafenCore(configured)
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.source = self.root / "听涛样本.wav"
        self.source.write_bytes(b"RIFF" + bytes(range(256)) * 400)

    def tearDown(self):
        self.temporary.cleanup()

    def test_roundtrip_progress_and_wrong_password(self):
        archive = self.root / "录音.bfq"
        recovered = self.root / "恢复.wav"
        progress = []
        result = self.core.pack(self.source, "local-only-password", archive, on_progress=lambda *args: progress.append(args))
        self.assertEqual(result.output, archive)
        self.assertEqual(result.filename, self.source.name)
        self.assertTrue(progress)
        self.core.unpack(archive, "local-only-password", recovered)
        self.assertEqual(recovered.read_bytes(), self.source.read_bytes())
        with self.assertRaisesRegex(BafenError, "口令|损坏"):
            self.core.unpack(archive, "wrong-password", self.root / "wrong.wav")
        self.assertFalse((self.root / "wrong.wav").exists())

    def test_cancel_leaves_no_output(self):
        cancelled = threading.Event()
        archive = self.root / "cancelled.bfq"

        def cancel_at_start(_processed, _total, _percent):
            cancelled.set()

        with self.assertRaisesRegex(BafenError, "取消"):
            self.core.pack(self.source, "password", archive, on_progress=cancel_at_start, cancel_event=cancelled)
        self.assertFalse(archive.exists())


if __name__ == "__main__":
    unittest.main()
