#!/usr/bin/env python3

import unittest
from unittest.mock import patch

import verify_cargo_workspace_manifests as verifier


MODEL_MANIFEST = verifier.CARGO_RS_ROOT / "tachyon-model" / "Cargo.toml"


def manifest_errors(path, manifest):
    with patch.object(verifier, "load_manifest", return_value=manifest):
        return verifier.manifest_errors(path, set(), set(), set(), set())


def valid_model_manifest():
    return {
        "package": {
            "name": "tachyon-model",
            "version": {"workspace": True},
            "edition": {"workspace": True},
            "license": {"workspace": True},
        },
        "lints": {"workspace": True},
    }


class VerifyCargoWorkspaceManifestsTests(unittest.TestCase):
    def test_tachyon_model_name_is_allowed(self):
        self.assertEqual(verifier.expected_package_name(MODEL_MANIFEST), "tachyon-model")
        self.assertEqual(manifest_errors(MODEL_MANIFEST, valid_model_manifest()), [])

    def test_tachyon_model_rejects_a_different_name(self):
        manifest = valid_model_manifest()
        manifest["package"]["name"] = "codex-tachyon-model"

        self.assertIn(
            "set `[package].name` to `tachyon-model` (found `codex-tachyon-model`)",
            manifest_errors(MODEL_MANIFEST, manifest),
        )

    def test_other_tachyon_directory_still_requires_codex_prefix(self):
        manifest_path = verifier.CARGO_RS_ROOT / "tachyon-example" / "Cargo.toml"
        manifest = valid_model_manifest()
        manifest["package"]["name"] = "tachyon-example"

        self.assertEqual(
            verifier.expected_package_name(manifest_path), "codex-tachyon-example"
        )
        self.assertIn(
            "set `[package].name` to `codex-tachyon-example` "
            "(found `tachyon-example`)",
            manifest_errors(manifest_path, manifest),
        )

    def test_model_still_requires_workspace_metadata_and_lints(self):
        errors = manifest_errors(MODEL_MANIFEST, {"package": {"name": "tachyon-model"}})

        self.assertCountEqual(
            errors,
            [
                "set `version.workspace = true` in `[package]`",
                "set `edition.workspace = true` in `[package]`",
                "set `license.workspace = true` in `[package]`",
                "add `[lints]` with `workspace = true`",
            ],
        )


if __name__ == "__main__":
    unittest.main()
