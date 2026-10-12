#!/usr/bin/env python3

import re
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = REPO_ROOT / ".github/workflows/bazel.yml"
SDK_WORKFLOW_PATH = REPO_ROOT / ".github/workflows/sdk.yml"
CACHE_HASH_INPUTS = (
    ".bazelversion",
    ".bazelrc",
    "MODULE.bazel",
    "MODULE.bazel.lock",
    "codex-rs/Cargo.lock",
    "codex-rs/rust-toolchain.toml",
)
SDK_CACHE_RESTORE = "Restore SDK Bazel action cache"


def named_step(job: str, name: str) -> str:
    """Return one named step from a workflow job, including its body."""
    match = re.search(
        rf"(?ms)^      - name: {re.escape(name)}\n(.*?)(?=^      - (?:name:|uses:)|\Z)",
        job,
    )
    if match is None:
        raise AssertionError(f"could not find workflow step: {name}")
    return match.group(0)


class LinuxTestActionCacheWorkflowTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
        cls.test_job = cls.workflow.split("  test-windows-shard:", 1)[0]
        cls.sdk_workflow = SDK_WORKFLOW_PATH.read_text(encoding="utf-8")
        cls.sdk_job = cls.sdk_workflow.split("  sdks:", 1)[1]

    def test_matrix_removes_duplicate_musl_row_and_preserves_other_coverage(
        self,
    ) -> None:
        matrix = self.test_job.split("        include:\n", 1)[1].split(
            "\n\n    runs-on:", 1
        )[0]
        active_rows = re.findall(
            r"(?m)^          - os: ([^\n]+)\n            target: ([^\n]+)", matrix
        )
        self.assertEqual(
            active_rows,
            [
                ("macos-15", "aarch64-apple-darwin"),
                ("macos-15-intel", "x86_64-apple-darwin"),
                ("ubuntu-24.04", "x86_64-unknown-linux-gnu"),
            ],
        )
        self.assertIn("No distinct musl Bazel platform is selected", matrix)
        self.assertIn("True musl coverage remains unvalidated", matrix)
        self.assertEqual(matrix.count("# - os: ubuntu-24.04-arm"), 2)

        windows_shard = self.workflow.split("  test-windows-shard:", 1)[1].split(
            "\n  test-windows:", 1
        )[0]
        native_windows = self.workflow.split("  test-windows-native-main:", 1)[1].split(
            "\n  clippy:", 1
        )[0]
        self.assertIn("target: x86_64-pc-windows-gnullvm", windows_shard)
        self.assertIn("BAZEL_TEST_SHARD_COUNT: 4", windows_shard)
        self.assertIn("needs: test-windows-shard", self.workflow)
        self.assertIn("target: x86_64-pc-windows-msvc", native_windows)

    def test_test_job_restores_the_sdk_cache_with_identical_configuration(self) -> None:
        test_configure = named_step(self.test_job, "Configure SDK Bazel action cache")
        sdk_configure = named_step(self.sdk_job, "Configure SDK Bazel action cache")
        test_restore = named_step(self.test_job, SDK_CACHE_RESTORE)
        sdk_restore = named_step(self.sdk_job, SDK_CACHE_RESTORE)

        for configure in (test_configure, sdk_configure):
            path_match = re.search(
                r'^\s+action_cache_path="([^"]+)"$', configure, re.M
            )
            self.assertIsNotNone(path_match)
            self.assertEqual(path_match.group(1), "${CI_BUILD_ROOT}/sdk-action-cache")
            self.assertIn('echo "path=${action_cache_path}"', configure)

        def configure_id(step: str) -> str:
            match = re.search(r"^        id: ([^\n]+)$", step, re.M)
            self.assertIsNotNone(match)
            return match.group(1)

        self.assertEqual(
            configure_id(test_configure),
            configure_id(sdk_configure),
        )

        self.assertIn("if: matrix.os == 'ubuntu-24.04'", test_configure)
        self.assertIn("if: matrix.os == 'ubuntu-24.04'", test_restore)
        self.assertIn("continue-on-error: true", test_restore)
        self.assertIn("continue-on-error: true", sdk_restore)

        for field in ("uses", "path", "key"):
            test_value = re.search(rf"^\s+{field}: (.*)$", test_restore, re.M)
            sdk_value = re.search(rf"^\s+{field}: (.*)$", sdk_restore, re.M)
            self.assertIsNotNone(test_value, f"missing {field} in test restore")
            self.assertIsNotNone(sdk_value, f"missing {field} in SDK restore")
            self.assertEqual(test_value.group(1), sdk_value.group(1), field)

        def restore_prefixes(step: str) -> list[str]:
            restore_keys = step.split("restore-keys: |", 1)[1]
            return re.findall(r"^\s+(\S.*)$", restore_keys, re.MULTILINE)

        self.assertEqual(restore_prefixes(test_restore), restore_prefixes(sdk_restore))
        hash_expression = "${{ hashFiles('" + "', '".join(CACHE_HASH_INPUTS) + "') }}"
        runner_arch_prefix = "sdk-actions-v1-${{ runner.os }}-${{ runner.arch }}-"
        expected_key = runner_arch_prefix + hash_expression + "-${{ github.sha }}"
        self.assertEqual(
            restore_prefixes(test_restore),
            [runner_arch_prefix + hash_expression + "-", runner_arch_prefix],
        )
        self.assertIn(
            "actions/cache/restore@668228422ae6a00e4ad889ee87cd7109ec5666a7",
            test_restore,
        )
        test_key = re.search(r"^\s+key: (.*)$", test_restore, re.M)
        self.assertIsNotNone(test_key)
        self.assertEqual(test_key.group(1), expected_key)
        self.assertNotIn("matrix.target", test_restore)

    def test_linux_only_cache_flags_preserve_test_targets_and_failures(self) -> None:
        test_step = named_step(self.test_job, "bazel test //...")
        linux_block = re.search(
            r'if \[\[ "\$\{RUNNER_OS:-\}" == "Linux" \]\]; then\n(.*?)\n\s+fi',
            test_step,
            re.S,
        )
        self.assertIsNotNone(linux_block)
        common_args = test_step.split(
            'if [[ "${RUNNER_OS:-}" == "Linux" ]]; then', 1
        )[0]
        cache_flags = (
            '"--disk_cache=${SDK_BAZEL_DISK_CACHE}"',
            "--experimental_disk_cache_gc_max_size=8G",
            "--experimental_disk_cache_gc_idle_delay=1s",
            "--cache_test_results=no",
        )
        for flag in cache_flags:
            with self.subTest(flag=flag):
                self.assertIn(flag, linux_block.group(1))
                self.assertNotIn(flag, common_args)

        self.assertIn(
            "SDK_BAZEL_DISK_CACHE: ${{ steps.sdk_action_cache.outputs.path }}",
            test_step,
        )
        self.assertIn("./.github/scripts/run-bazel-ci.sh", test_step)
        self.assertNotIn("|| true", test_step)
        self.assertNotIn("continue-on-error:", test_step)

        targets = test_step.split("bazel_targets=(", 1)[1].split(")", 1)[0]
        target_arguments = [
            line.strip()
            for line in targets.splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        ]
        self.assertEqual(
            target_arguments,
            [
                "//...",
                "-//third_party/v8:all",
                "-//codex-rs/v8-poc:v8-poc-unit-tests",
            ],
        )

    def test_test_job_does_not_save_compiler_cache_but_keeps_repo_cache_save(
        self,
    ) -> None:
        self.assertNotIn("Save SDK Bazel action cache", self.test_job)
        self.assertNotIn("Check SDK Bazel action cache size", self.test_job)
        self.assertEqual(self.test_job.count("uses: actions/cache/save@"), 1)

        repository_save = named_step(self.test_job, "Save bazel repository cache")
        self.assertIn("uses: actions/cache/save@", repository_save)
        self.assertIn(
            "path: ${{ steps.prepare_bazel.outputs.repository-cache-path }}",
            repository_save,
        )
        self.assertNotIn("sdk-action-cache", repository_save)

        test_step = named_step(self.test_job, "bazel test //...")
        self.assertNotIn("continue-on-error:", test_step)


if __name__ == "__main__":
    unittest.main()
