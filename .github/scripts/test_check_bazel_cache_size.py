#!/usr/bin/env python3

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT_PATH = Path(__file__).with_name("check_bazel_cache_size.py")
SPEC = importlib.util.spec_from_file_location("check_bazel_cache_size", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
cache_size = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cache_size)


class CheckBazelCacheSizeTest(unittest.TestCase):
    def test_empty_directory_is_not_eligible(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            should_save, reason, total_bytes, entry_count = cache_size.cache_save_decision(
                Path(temp_dir), 100
            )

        self.assertFalse(should_save)
        self.assertEqual(reason, "cache is empty")
        self.assertEqual(total_bytes, 0)
        self.assertEqual(entry_count, 0)

    def test_missing_directory_is_not_eligible(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            missing_path = Path(temp_dir) / "missing-cache"
            should_save, reason, total_bytes, entry_count = cache_size.cache_save_decision(
                missing_path, 100
            )

        self.assertFalse(should_save)
        self.assertIn("could not inspect cache", reason)
        self.assertEqual(total_bytes, 0)
        self.assertEqual(entry_count, 0)

    def test_zero_byte_file_counts_as_a_cache_entry(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            (Path(temp_dir) / "empty-action-result").touch()
            should_save, reason, total_bytes, entry_count = cache_size.cache_save_decision(
                Path(temp_dir), 100
            )

        self.assertTrue(should_save, reason)
        self.assertEqual(total_bytes, 0)
        self.assertEqual(entry_count, 1)

    def test_oversized_directory_is_not_eligible(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            (Path(temp_dir) / "action-result").write_bytes(b"oversized")
            should_save, reason, total_bytes, entry_count = cache_size.cache_save_decision(
                Path(temp_dir), 2
            )

        self.assertFalse(should_save)
        self.assertIn("exceeds", reason)
        self.assertGreater(total_bytes, 2)
        self.assertEqual(entry_count, 1)

    def test_directory_at_size_limit_is_eligible(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            (Path(temp_dir) / "action-result").write_bytes(b"four")
            should_save, reason, total_bytes, entry_count = cache_size.cache_save_decision(
                Path(temp_dir), 4
            )

        self.assertTrue(should_save, reason)
        self.assertEqual(total_bytes, 4)
        self.assertEqual(entry_count, 1)

    def test_symlink_is_counted_without_following_target(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir) / "cache"
            root.mkdir()
            target = Path(temp_dir) / "outside-cache"
            target.write_bytes(b"a much larger target payload")
            link = root / "cache-link"
            try:
                link.symlink_to(target)
            except (NotImplementedError, OSError) as error:
                self.skipTest(f"symlinks are unavailable: {error}")

            total_bytes, entry_count = cache_size.measure_cache(root)
            link_size = link.lstat().st_size
            target_size = target.stat().st_size

        self.assertEqual(entry_count, 1)
        self.assertEqual(total_bytes, link_size)
        self.assertNotEqual(total_bytes, target_size)

    def test_unreadable_cache_emits_save_false(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir) / "cache"
            root.mkdir()
            output_path = Path(temp_dir) / "github-output"
            with mock.patch.object(cache_size.os, "walk", side_effect=PermissionError("denied")):
                with mock.patch.dict(os.environ, {"GITHUB_OUTPUT": str(output_path)}):
                    status = cache_size.main([str(SCRIPT_PATH), str(root)])

            self.assertEqual(status, 0)
            self.assertEqual(output_path.read_text(encoding="utf-8"), "save=false\n")

    def test_cli_appends_save_decisions_for_eligible_and_missing_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            cache_path = root / "cache"
            cache_path.mkdir()
            (cache_path / "zero-byte-entry").touch()
            eligible_output = root / "eligible-output"
            missing_output = root / "missing-output"

            for path, output_path, expected in (
                (cache_path, eligible_output, "save=true\n"),
                (root / "missing-cache", missing_output, "save=false\n"),
            ):
                env = os.environ.copy()
                env["GITHUB_OUTPUT"] = str(output_path)
                result = subprocess.run(
                    [sys.executable, str(SCRIPT_PATH), str(path)],
                    env=env,
                    check=False,
                    capture_output=True,
                    text=True,
                )
                with self.subTest(path=path):
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(output_path.read_text(encoding="utf-8"), expected)


if __name__ == "__main__":
    unittest.main()
