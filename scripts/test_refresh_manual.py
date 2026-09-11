"""Failure boundaries for the manual refresh; real rendering is checked on macOS."""

from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import refresh_manual as manual


class RefreshManualTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.source = self.root / "source"
        self.output = self.root / "website/manual"
        (self.source / "images").mkdir(parents=True)
        self.output.mkdir(parents=True)
        (self.source / "index.md").write_text("# Manual\n\n![Grid](images/grid.png)\n")
        (self.source / "images/grid.png").write_bytes(b"previous image")
        (self.output / "index.html").write_text("previous HTML")

    def assert_previous_unchanged(self):
        self.assertEqual((self.source / "images/grid.png").read_bytes(), b"previous image")
        self.assertEqual((self.output / "index.html").read_text(), "previous HTML")

    def test_requires_a_unique_recipe_even_when_old_image_exists(self):
        for reference in ("images/grid.png", "images/../grid.png", "images//grid.png", "/images/grid.png"):
            with self.subTest(reference=reference), self.assertRaises(ValueError):
                manual.plan_images(self.source, [reference], {})
        (self.source / "diagrams").mkdir()
        (self.source / "diagrams/grid.svg").write_text("<svg/>")
        with self.assertRaisesRegex(ValueError, "both a capture recipe"):
            manual.plan_images(self.source, ["images/grid.png"], {"grid": {}})
        self.assert_previous_unchanged()

    def test_generation_and_export_failures_leave_both_readers_untouched(self):
        def capture(binary, entry, output):
            output.write_bytes(b"new staged image")

        def failed_capture(binary, entry, output):
            capture(binary, entry, output)
            raise RuntimeError("capture failed after writing a partial image")

        for failure in ("capture", "export"):
            with self.subTest(failure=failure), \
                    patch.object(manual, "load_captures", return_value={"grid": {}}), \
                    patch.object(manual, "build_binary", return_value=Path("/test/tool")), \
                    patch.object(manual, "discover_images", return_value=["images/grid.png"]), \
                    patch.object(manual, "capture_image", side_effect=capture) as generate, \
                    patch.object(manual.subprocess, "run") as export:
                if failure == "capture":
                    generate.side_effect = failed_capture
                else:
                    export.side_effect = subprocess.CalledProcessError(1, "export")
                with self.assertRaises((RuntimeError, subprocess.CalledProcessError)):
                    manual.refresh(self.source, self.output)
                if failure == "capture":
                    export.assert_not_called()
                self.assert_previous_unchanged()

    def test_copies_only_generated_referenced_images_after_export(self):
        (self.source / "images/unused.png").write_bytes(b"keep unused")

        def capture(binary, entry, output):
            output.write_bytes(b"fresh capture")

        def export(command, **kwargs):
            staged = Path(command[command.index("--source") + 1])
            self.assertFalse((staged / "images/unused.png").exists())
            self.assertEqual((staged / "images/grid.png").read_bytes(), b"fresh capture")
            self.assert_previous_unchanged()

        with patch.object(manual, "load_captures", return_value={"grid": {}, "unused": {}}), \
                patch.object(manual, "build_binary", return_value=Path("/test/tool")), \
                patch.object(manual, "discover_images", return_value=["images/grid.png"]), \
                patch.object(manual, "capture_image", side_effect=capture) as generate, \
                patch.object(manual.subprocess, "run", side_effect=export):
            manual.refresh(self.source, self.output)
            generate.assert_called_once()
        self.assertEqual((self.source / "images/grid.png").read_bytes(), b"fresh capture")
        self.assertEqual((self.source / "images/unused.png").read_bytes(), b"keep unused")


if __name__ == "__main__":
    unittest.main()
