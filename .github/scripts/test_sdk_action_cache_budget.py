#!/usr/bin/env python3

import os
import re
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = REPO_ROOT / ".github/workflows/sdk.yml"
STEP_NAME = "Check SDK Bazel action cache size"
MAX_CACHE_SIZE_BYTES = 8 * 1024 * 1024 * 1024


def extract_run_block() -> tuple[str, str]:
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    step_match = re.search(
        rf"(?ms)^      - name: {re.escape(STEP_NAME)}\n(.*?)(?=^      - (?:name:|uses:)|\Z)",
        workflow,
    )
    if step_match is None:
        raise AssertionError(f"could not find workflow step: {STEP_NAME}")

    step = step_match.group(0)
    run_marker = "        run: |\n"
    if run_marker not in step:
        raise AssertionError(f"could not find run block in workflow step: {STEP_NAME}")
    return step, textwrap.dedent(step.split(run_marker, 1)[1])


class SDKActionCacheBudgetTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.step, cls.shell = extract_run_block()

    def run_size_guard(
        self, *, size_bytes: int, empty: bool = False, du_fails: bool = False
    ) -> tuple[subprocess.CompletedProcess[str], str, str, bool]:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            build_root = root / "build"
            cache_path = build_root / "sdk-action-cache"
            cache_path.mkdir(parents=True)
            if not empty:
                (cache_path / "one-byte-entry").write_bytes(b"x")

            output_path = root / "github-output"
            summary_path = root / "step-summary"
            output_path.touch()
            summary_path.touch()

            mock_bin = root / "bin"
            mock_bin.mkdir()
            du_called_path = root / "du-called"
            mock_du = mock_bin / "du"
            mock_du.write_text(
                "#!/bin/sh\n"
                'printf called > "$MOCK_DU_CALLED"\n'
                'if [ "$MOCK_DU_FAIL" = "1" ]; then exit 23; fi\n'
                'printf "%s\\t%s\\n" "$MOCK_DU_BYTES" "$3"\n',
                encoding="utf-8",
            )
            mock_du.chmod(0o755)

            env = {
                "PATH": f"{mock_bin}:/usr/bin:/bin",
                "CI_BUILD_ROOT": str(build_root),
                "GITHUB_OUTPUT": str(output_path),
                "GITHUB_STEP_SUMMARY": str(summary_path),
                "MOCK_DU_BYTES": str(size_bytes),
                "MOCK_DU_FAIL": "1" if du_fails else "0",
                "MOCK_DU_CALLED": str(du_called_path),
            }
            result = subprocess.run(
                ["/bin/bash", "-c", self.shell],
                cwd=REPO_ROOT,
                env=env,
                check=False,
                capture_output=True,
                text=True,
            )
            return (
                result,
                output_path.read_text(encoding="utf-8"),
                summary_path.read_text(encoding="utf-8"),
                du_called_path.exists(),
            )

    def test_below_and_at_limit_are_eligible(self) -> None:
        for size_bytes in (MAX_CACHE_SIZE_BYTES - 1, MAX_CACHE_SIZE_BYTES):
            with self.subTest(size_bytes=size_bytes):
                result, output, summary, du_called = self.run_size_guard(
                    size_bytes=size_bytes
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(output, "save=true\n")
                self.assertIn("eligible for upload (8 GiB budget)", summary)
                self.assertTrue(du_called)

    def test_above_limit_is_not_eligible(self) -> None:
        result, output, summary, du_called = self.run_size_guard(
            size_bytes=MAX_CACHE_SIZE_BYTES + 1
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "save=false\n")
        self.assertIn("exceeds the 8 GiB upload budget", summary)
        self.assertTrue(du_called)

    def test_empty_cache_is_not_eligible_without_measuring(self) -> None:
        result, output, summary, du_called = self.run_size_guard(
            size_bytes=0, empty=True, du_fails=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "save=false\n")
        self.assertIn("cache is empty", summary)
        self.assertFalse(du_called)

    def test_du_failure_is_nonzero_without_save_true(self) -> None:
        self.assertIn("continue-on-error: true", self.step)
        result, output, _summary, du_called = self.run_size_guard(
            size_bytes=1, du_fails=True
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("save=true", output)
        self.assertTrue(du_called)


if __name__ == "__main__":
    unittest.main()
