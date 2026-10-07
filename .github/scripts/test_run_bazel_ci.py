#!/usr/bin/env python3

import json
import os
import subprocess
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory


RUN_BAZEL_CI = Path(__file__).with_name("run-bazel-ci.sh")


class RunBazelCiTest(unittest.TestCase):
    def run_ci(
        self,
        temp_dir: str,
        args: list[str],
        *,
        runner_os: str = "Linux",
        api_key: str | None = None,
        bazel_status: int = 0,
    ) -> tuple[subprocess.CompletedProcess[str], list[list[str]]]:
        stub_path = Path(temp_dir) / "bazel-stub"
        calls_path = Path(temp_dir) / "bazel-calls.jsonl"
        stub_path.write_text(
            """#!/usr/bin/env python3
import json
import os
import sys

arguments = sys.argv[1:]
with open(os.environ["CODEX_BAZEL_CALLS"], "a", encoding="utf-8") as calls_file:
    calls_file.write(json.dumps(arguments) + "\\n")

if "info" in arguments and "bazel-testlogs" in arguments:
    print("/tmp/bazel-testlogs")
    raise SystemExit(0)

if "test" in arguments:
    print("FAIL: //codex-rs/cli:tests (see /tmp/bazel-testlogs/cli/tests/test.log)")

raise SystemExit(int(os.environ["FAKE_BAZEL_STATUS"]))
""",
            encoding="utf-8",
        )
        stub_path.chmod(0o755)

        env = os.environ.copy()
        for name in (
            "BUILDBUDDY_API_KEY",
            "GITHUB_ACTIONS",
            "GITHUB_EVENT_NAME",
            "GITHUB_EVENT_PATH",
            "GITHUB_REPOSITORY",
            "BAZEL_OUTPUT_USER_ROOT",
            "BAZEL_REPO_CONTENTS_CACHE",
            "BAZEL_REPOSITORY_CACHE",
            "CODEX_BAZEL_EXECUTION_LOG_COMPACT_DIR",
        ):
            env.pop(name, None)
        env.update(
            {
                "CODEX_BAZEL_BIN": str(stub_path),
                "CODEX_BAZEL_CALLS": str(calls_path),
                "FAKE_BAZEL_STATUS": str(bazel_status),
                "RUNNER_OS": runner_os,
            }
        )
        if api_key is not None:
            env["BUILDBUDDY_API_KEY"] = api_key
        if runner_os == "Windows":
            env["CODEX_BAZEL_WINDOWS_PATH"] = r"C:\Program Files\Git\usr\bin"

        result = subprocess.run(
            ["bash", str(RUN_BAZEL_CI), *args],
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        calls = [
            json.loads(line)
            for line in calls_path.read_text(encoding="utf-8").splitlines()
        ]
        return result, calls

    @staticmethod
    def command_calls(calls: list[list[str]], command: str) -> list[list[str]]:
        return [call for call in calls if command in call]

    def test_keyless_linux_build_test_and_queries_use_ci_local(self) -> None:
        for command in ("build", "test", "cquery", "aquery"):
            with self.subTest(command=command), TemporaryDirectory() as temp_dir:
                bazel_args = [command]
                if command in ("cquery", "aquery"):
                    bazel_args.extend(
                        ["--output=label", "deps(//codex-rs/cli:codex)"]
                    )
                result, calls = self.run_ci(
                    temp_dir,
                    ["--", *bazel_args, "--", "//codex-rs/cli:codex"],
                )

                self.assertEqual(result.returncode, 0, result.stderr)
                command_calls = self.command_calls(calls, command)
                self.assertEqual(len(command_calls), 1, calls)
                invocation = command_calls[0]
                self.assertIn("--config=ci-local", invocation)
                self.assertNotIn("--config=ci-linux", invocation)
                self.assertFalse(
                    any(arg.startswith("--config=buildbuddy-") for arg in invocation)
                )

    def test_keyless_failed_test_uses_ci_local_for_output_resolution(self) -> None:
        with TemporaryDirectory() as temp_dir:
            result, calls = self.run_ci(
                temp_dir,
                [
                    "--print-failed-test-logs",
                    "--",
                    "test",
                    "--test_tag_filters=-argument-comment-lint",
                    "--",
                    "//codex-rs/cli:tests",
                ],
                bazel_status=23,
            )

            self.assertEqual(result.returncode, 23, result.stderr)
            test_call = self.command_calls(calls, "test")
            info_call = self.command_calls(calls, "info")
            self.assertEqual(len(test_call), 1, calls)
            self.assertEqual(len(info_call), 1, calls)
            self.assertIn("--config=ci-local", test_call[0])
            self.assertIn("--config=ci-local", info_call[0])

    def test_keyless_windows_cross_compile_uses_gnullvm_host_and_exec_platforms(self) -> None:
        with TemporaryDirectory() as temp_dir:
            result, calls = self.run_ci(
                temp_dir,
                [
                    "--windows-cross-compile",
                    "--",
                    "test",
                    "--",
                    "//codex-rs/cli:tests",
                ],
                runner_os="Windows",
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            invocation = self.command_calls(calls, "test")[0]
            self.assertIn("--config=ci-local", invocation)
            self.assertNotIn("--config=ci-windows-cross", invocation)
            self.assertIn("--host_platform=//:local_windows", invocation)
            self.assertIn("--platforms=//:windows_x86_64_gnullvm", invocation)
            self.assertIn(
                "--extra_execution_platforms=//:windows_x86_64_gnullvm", invocation
            )
            self.assertIn(
                "--extra_toolchains=//:windows_gnullvm_tests_on_gnullvm_host_toolchain",
                invocation,
            )
            self.assertIn("--jobs=8", invocation)
            self.assertFalse(any("//:rbe" in arg for arg in invocation))
            self.assertNotIn("--host_platform=//:local_windows_msvc", invocation)
            self.assertNotIn(
                "--extra_execution_platforms=//:windows_x86_64_msvc", invocation
            )

    def test_keyless_windows_cross_compile_preserves_explicit_msvc_host(self) -> None:
        with TemporaryDirectory() as temp_dir:
            result, calls = self.run_ci(
                temp_dir,
                [
                    "--windows-cross-compile",
                    "--windows-msvc-host-platform",
                    "--",
                    "test",
                    "--",
                    "//codex-rs/cli:tests",
                ],
                runner_os="Windows",
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            invocation = self.command_calls(calls, "test")[0]
            self.assertIn("--host_platform=//:local_windows_msvc", invocation)
            self.assertIn("--platforms=//:windows_x86_64_gnullvm", invocation)
            self.assertIn(
                "--extra_execution_platforms=//:windows_x86_64_msvc", invocation
            )
            self.assertIn(
                "--extra_toolchains=//:windows_gnullvm_tests_on_msvc_host_toolchain",
                invocation,
            )
            self.assertNotIn("--host_platform=//:local_windows", invocation)

    def test_keyless_windows_cross_compile_respects_caller_platform_overrides(self) -> None:
        explicit_args = [
            "--host_platform=//:custom_host",
            "--platforms=//:custom_target",
            "--extra_execution_platforms=//:custom_exec",
            "--extra_toolchains=//:custom_test_toolchain",
        ]
        with TemporaryDirectory() as temp_dir:
            result, calls = self.run_ci(
                temp_dir,
                [
                    "--windows-cross-compile",
                    "--",
                    "build",
                    *explicit_args,
                    "--",
                    "//codex-rs/cli:codex",
                ],
                runner_os="Windows",
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            invocation = self.command_calls(calls, "build")[0]
            for arg in explicit_args:
                self.assertIn(arg, invocation)
            self.assertNotIn("--host_platform=//:local_windows", invocation)
            self.assertNotIn("--platforms=//:windows_x86_64_gnullvm", invocation)
            self.assertNotIn(
                "--extra_execution_platforms=//:windows_x86_64_gnullvm", invocation
            )
            self.assertNotIn(
                "--extra_toolchains=//:windows_gnullvm_tests_on_gnullvm_host_toolchain",
                invocation,
            )

    def test_keyless_windows_cross_compile_respects_split_form_platform_overrides(self) -> None:
        explicit_args = [
            "--host_platform",
            "//:custom_host",
            "--platforms",
            "//:custom_target",
            "--extra_execution_platforms",
            "//:custom_exec",
            "--extra_toolchains",
            "//:custom_test_toolchain",
        ]
        with TemporaryDirectory() as temp_dir:
            result, calls = self.run_ci(
                temp_dir,
                [
                    "--windows-cross-compile",
                    "--",
                    "build",
                    *explicit_args,
                    "--",
                    "//codex-rs/cli:codex",
                ],
                runner_os="Windows",
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            invocation = self.command_calls(calls, "build")[0]
            for arg in explicit_args:
                self.assertIn(arg, invocation)
            self.assertNotIn("--host_platform=//:local_windows", invocation)
            self.assertNotIn("--platforms=//:windows_x86_64_gnullvm", invocation)
            self.assertNotIn(
                "--extra_execution_platforms=//:windows_x86_64_gnullvm", invocation
            )
            self.assertNotIn(
                "--extra_toolchains=//:windows_gnullvm_tests_on_gnullvm_host_toolchain",
                invocation,
            )

    def test_explicit_msvc_host_preserves_split_form_caller_host_platform(self) -> None:
        with TemporaryDirectory() as temp_dir:
            result, calls = self.run_ci(
                temp_dir,
                [
                    "--windows-msvc-host-platform",
                    "--",
                    "build",
                    "--host_platform",
                    "//:custom_host",
                    "--",
                    "//codex-rs/cli:codex",
                ],
                runner_os="Windows",
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            invocation = self.command_calls(calls, "build")[0]
            self.assertIn("--host_platform", invocation)
            self.assertIn("//:custom_host", invocation)
            self.assertNotIn("--host_platform=//:local_windows_msvc", invocation)

    def test_keyed_windows_cross_compile_keeps_rbe_configuration(self) -> None:
        with TemporaryDirectory() as temp_dir:
            result, calls = self.run_ci(
                temp_dir,
                [
                    "--windows-cross-compile",
                    "--",
                    "build",
                    "--",
                    "//codex-rs/cli:codex",
                ],
                runner_os="Windows",
                api_key="test-token",
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            invocation = self.command_calls(calls, "build")[0]
            self.assertIn("--config=ci-windows-cross", invocation)
            self.assertIn("--host_platform=//:rbe", invocation)
            self.assertIn("--shell_executable=/bin/bash", invocation)
            self.assertNotIn("--config=ci-local", invocation)
            self.assertNotIn("--host_platform=//:local_windows", invocation)
            self.assertFalse(
                any(arg.startswith("--extra_execution_platforms=") for arg in invocation)
            )

    def test_keyless_windows_build_test_and_query_failures_preserve_exit_status(self) -> None:
        for command in ("build", "test", "cquery"):
            with self.subTest(command=command), TemporaryDirectory() as temp_dir:
                bazel_args = [command]
                if command == "cquery":
                    bazel_args.extend(
                        ["--output=label", "deps(//codex-rs/cli:codex)"]
                    )
                result, calls = self.run_ci(
                    temp_dir,
                    [
                        "--windows-cross-compile",
                        "--",
                        *bazel_args,
                        "--",
                        "//codex-rs/cli:codex",
                    ],
                    runner_os="Windows",
                    bazel_status=37,
                )

                self.assertEqual(result.returncode, 37, result.stderr)
                invocation = self.command_calls(calls, command)
                self.assertEqual(len(invocation), 1, calls)
                self.assertIn("--host_platform=//:local_windows", invocation[0])

    def test_keyed_linux_invocation_keeps_remote_ci_configuration(self) -> None:
        with TemporaryDirectory() as temp_dir:
            result, calls = self.run_ci(
                temp_dir,
                ["--", "build", "--", "//codex-rs/cli:codex"],
                api_key="test-token",
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            invocation = self.command_calls(calls, "build")[0]
            self.assertIn("--config=buildbuddy-generic-rbe", invocation)
            self.assertIn("--config=ci-linux", invocation)
            self.assertNotIn("--config=ci-local", invocation)

    def test_explicit_bazel_args_and_nonzero_exit_status_are_preserved(self) -> None:
        with TemporaryDirectory() as temp_dir:
            explicit_args = [
                "--host_platform=//:custom_host",
                "--platforms=//:custom_target",
                "--define=GREETING=hello world",
            ]
            result, calls = self.run_ci(
                temp_dir,
                ["--", "build", *explicit_args, "--", "//codex-rs/cli:codex"],
                bazel_status=37,
            )

            self.assertEqual(result.returncode, 37, result.stderr)
            invocation = self.command_calls(calls, "build")[0]
            self.assertIn("--config=ci-local", invocation)
            for arg in explicit_args:
                self.assertIn(arg, invocation)

    def test_explicit_disk_cache_value_is_last_after_ci_config(self) -> None:
        cases = (
            (None, "ci-local", ["--disk_cache="], "--disk_cache="),
            (
                None,
                "ci-local",
                ["--disk_cache=/tmp/caller-cache"],
                "--disk_cache=/tmp/caller-cache",
            ),
            (
                "test-token",
                "ci-linux",
                ["--disk_cache=/tmp/caller-cache"],
                "--disk_cache=/tmp/caller-cache",
            ),
            ("test-token", "ci-linux", ["--disk_cache="], "--disk_cache="),
            (
                None,
                "ci-local",
                ["--disk_cache=/tmp/first", "--disk_cache=/tmp/last"],
                "--disk_cache=/tmp/last",
            ),
        )

        for api_key, ci_config, disk_cache_args, expected_disk_cache in cases:
            with self.subTest(
                ci_config=ci_config, disk_cache_args=disk_cache_args
            ), TemporaryDirectory() as temp_dir:
                result, calls = self.run_ci(
                    temp_dir,
                    ["--", "build", *disk_cache_args, "--", "//codex-rs/cli:codex"],
                    api_key=api_key,
                )

                self.assertEqual(result.returncode, 0, result.stderr)
                invocation = self.command_calls(calls, "build")[0]
                cache_args = [
                    (index, arg)
                    for index, arg in enumerate(invocation)
                    if arg.startswith("--disk_cache=")
                ]
                self.assertGreaterEqual(
                    len(cache_args), len(disk_cache_args) + 1, invocation
                )
                self.assertEqual(cache_args[-1][1], expected_disk_cache, invocation)
                self.assertGreater(
                    cache_args[-1][0],
                    invocation.index(f"--config={ci_config}"),
                    invocation,
                )


if __name__ == "__main__":
    unittest.main()
