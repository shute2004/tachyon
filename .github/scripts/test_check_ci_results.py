#!/usr/bin/env python3

"""Subprocess tests for the blocking CI dependency-result checker."""

import json
import os
from pathlib import Path
import subprocess
import sys
import unittest


CHECKER = Path(__file__).with_name("check_ci_results.py")


class CheckCiResultsTests(unittest.TestCase):
    def run_checker(self, needs: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(CHECKER)],
            env={**os.environ, "NEEDS": needs},
            capture_output=True,
            text=True,
            check=False,
        )

    def test_all_successful_dependencies_pass(self) -> None:
        needs = {
            "build": {"result": "success"},
            "lint": {"result": "success"},
        }

        result = self.run_checker(json.dumps(needs))

        self.assertEqual(result.returncode, 0, result.stderr)

    def test_unsuccessful_dependencies_fail(self) -> None:
        for conclusion in ("failure", "cancelled", "skipped"):
            with self.subTest(conclusion=conclusion):
                needs = {"dependency": {"result": conclusion}}

                result = self.run_checker(json.dumps(needs))

                self.assertNotEqual(result.returncode, 0)

    def test_malformed_needs_fail(self) -> None:
        result = self.run_checker("{")

        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
