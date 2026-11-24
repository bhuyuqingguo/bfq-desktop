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

    def test_folder_compression_verify_and_rename(self):
        folder = self.root / "资料目录"
        folder.mkdir()
        (folder / "内容.txt").write_text("folder roundtrip", encoding="utf-8")
        archive = self.root / "目录.bfq"
        self.core.pack(folder, "password", archive, compress=True)
        checked = self.core.verify(archive, "password")
        self.assertEqual(checked.filename, "资料目录.zip")
        restored = self.core.unpack(archive, "password", self.root / "恢复目录")
        self.assertEqual((restored.output / "内容.txt").read_text(encoding="utf-8"), "folder roundtrip")
        renamed = self.core.unpack(archive, "password", self.root / "恢复目录", rename_existing=True)
        self.assertEqual(renamed.output, self.root / "恢复目录-1")

    def test_cancel_leaves_no_output(self):
        cancelled = threading.Event()
        archive = self.root / "cancelled.bfq"

        cancelled.set()
        with self.assertRaisesRegex(BafenError, "取消"):
            self.core.pack(self.source, "password", archive, cancel_event=cancelled)
        self.assertFalse(archive.exists())


if __name__ == "__main__":
    unittest.main()
