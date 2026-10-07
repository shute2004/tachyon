#!/usr/bin/env python3

"""Bound the size of a local Bazel action cache before uploading it."""

import os
import stat
import sys
from pathlib import Path


MAX_CACHE_SIZE_BYTES = 2 * 1024 * 1024 * 1024


def measure_cache(path: Path) -> tuple[int, int]:
    """Return (apparent bytes, entry count) without following symlinks."""
    root_mode = path.lstat().st_mode
    if not stat.S_ISDIR(root_mode):
        raise OSError(f"cache path is not a directory: {path}")

    total_bytes = 0
    entry_count = 0

    def raise_walk_error(error: OSError) -> None:
        raise error

    for root, directory_names, file_names in os.walk(
        path,
        topdown=True,
        onerror=raise_walk_error,
        followlinks=False,
    ):
        root_path = Path(root)
        for name in list(directory_names):
            entry_path = root_path / name
            entry_stat = entry_path.lstat()
            if stat.S_ISLNK(entry_stat.st_mode):
                directory_names.remove(name)
                total_bytes += entry_stat.st_size
                entry_count += 1

        for name in file_names:
            entry_stat = (root_path / name).lstat()
            if stat.S_ISREG(entry_stat.st_mode) or stat.S_ISLNK(entry_stat.st_mode):
                total_bytes += entry_stat.st_size
                entry_count += 1

    return total_bytes, entry_count


def cache_save_decision(path: Path, max_size_bytes: int) -> tuple[bool, str, int, int]:
    try:
        total_bytes, entry_count = measure_cache(path)
    except OSError as error:
        return False, f"could not inspect cache: {error}", 0, 0

    if entry_count == 0:
        return False, "cache is empty", total_bytes, entry_count
    if total_bytes > max_size_bytes:
        return (
            False,
            f"cache exceeds the {max_size_bytes}-byte upload budget",
            total_bytes,
            entry_count,
        )
    return True, "cache is within the upload budget", total_bytes, entry_count


def main(argv: list[str] | None = None) -> int:
    args = sys.argv if argv is None else argv
    if len(args) != 2:
        print(f"usage: {Path(args[0]).name} CACHE_PATH", file=sys.stderr)
        return 2

    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        print("GITHUB_OUTPUT is not set", file=sys.stderr)
        return 2

    should_save, reason, total_bytes, entry_count = cache_save_decision(
        Path(args[1]), MAX_CACHE_SIZE_BYTES
    )
    try:
        with Path(output_path).open("a", encoding="utf-8") as output_file:
            output_file.write(f"save={'true' if should_save else 'false'}\n")
    except OSError as error:
        print(f"could not write cache decision: {error}", file=sys.stderr)
        return 1

    if should_save:
        print(
            f"Bazel action cache is {total_bytes} bytes across {entry_count} entries; "
            f"eligible for upload (2 GiB budget)."
        )
    else:
        print(
            f"Skipping Bazel action cache upload: {reason} "
            f"({total_bytes} bytes, {entry_count} entries)."
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
