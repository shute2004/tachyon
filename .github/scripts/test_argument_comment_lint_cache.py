#!/usr/bin/env python3

import re
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
ACTION_PATH = REPO_ROOT / ".github/actions/run-argument-comment-lint/action.yml"


class ArgumentCommentLintCacheWorkflowTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.action = ACTION_PATH.read_text(encoding="utf-8")

    def test_action_prepares_repository_and_local_action_caches(self) -> None:
        self.assertIn("uses: ./.github/actions/prepare-bazel-ci", self.action)
        self.assertIn("cache-scope: argument-comment-lint", self.action)
        self.assertIn('action_cache_path="${CI_BUILD_ROOT}/lint-action-cache"', self.action)
        self.assertIn("continue-on-error: true\n      uses: actions/cache/restore@", self.action)
        self.assertIn("continue-on-error: true\n      uses: actions/cache/save@", self.action)
        self.assertIn("steps.prepare_bazel.outputs.repository-cache-hit != 'true'", self.action)

    def test_local_cache_key_separates_platform_configuration_and_revision(self) -> None:
        self.assertIn("lint-actions-v1-${{ runner.os }}-${{ runner.arch }}-${{ inputs.target }}-", self.action)
        self.assertIn("${{ github.sha }}", self.action)
        expected_inputs = (
            ".bazelversion",
            ".bazelrc",
            "MODULE.bazel",
            "MODULE.bazel.lock",
            "codex-rs/Cargo.lock",
            "codex-rs/rust-toolchain.toml",
            "tools/argument-comment-lint/Cargo.lock",
            "tools/argument-comment-lint/rust-toolchain",
        )
        for cache_input in expected_inputs:
            with self.subTest(cache_input=cache_input):
                self.assertIn(cache_input, self.action)

        restore_section = self.action.split("- name: Restore argument comment lint Bazel action cache", 1)[1]
        restore_section = restore_section.split("- name: Install Linux sandbox build dependencies", 1)[0]
        restore_keys = re.findall(r"^\s+lint-actions-v1-.*$", restore_section, re.MULTILINE)
        self.assertEqual(len(restore_keys), 1)
        self.assertTrue(restore_keys[0].endswith(" }}-"))

    def test_all_platform_lint_commands_use_the_bounded_cache(self) -> None:
        self.assertEqual(self.action.count('"--disk_cache=${ARGUMENT_COMMENT_LINT_BAZEL_DISK_CACHE}"'), 2)
        self.assertEqual(self.action.count('"--experimental_disk_cache_gc_max_size=2G"'), 2)
        self.assertEqual(self.action.count('"--experimental_disk_cache_gc_idle_delay=1s"'), 2)
        self.assertIn("tools/argument-comment-lint/list-bazel-targets.sh", self.action)
        self.assertIn("run-argument-comment-lint-bazel.sh", self.action)
        self.assertIn("--platforms=//:local_windows", self.action)
        self.assertIn("--config=argument-comment-lint", self.action)

    def test_cache_upload_is_bounded_nonfatal_and_fork_safe(self) -> None:
        self.assertIn("check_bazel_cache_size.py", self.action)
        self.assertIn("lint_action_cache_size.outputs.save == 'true'", self.action)
        self.assertIn("steps.restore_lint_action_cache.outputs.cache-hit != 'true'", self.action)
        self.assertIn("github.event_name != 'pull_request'", self.action)
        self.assertIn("github.event.pull_request.head.repo.full_name == github.repository", self.action)
        self.assertRegex(
            self.action,
            r"(?s)- name: Save argument comment lint Bazel action cache.*?continue-on-error: true",
        )

    def test_lint_failures_are_not_masked_and_logs_are_uploaded_nonfatally(self) -> None:
        lint_steps = self.action.split("- name: Run argument comment lint on codex-rs via Bazel")
        self.assertEqual(len(lint_steps), 3)
        for lint_step in lint_steps[1:]:
            first_step = lint_step.split("\n    - name:", 1)[0]
            self.assertNotIn("continue-on-error: true", first_step)
        self.assertIn("Upload argument comment lint Bazel execution logs", self.action)
        self.assertIn("continue-on-error: true\n      uses: actions/upload-artifact@", self.action)


if __name__ == "__main__":
    unittest.main()
