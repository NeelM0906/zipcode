"""Exercise prompt delivery without changing a user's installation."""

import json
from pathlib import Path
import tempfile
import tomllib
import unittest

from bundle import build_bundle


class BundleTests(unittest.TestCase):
    def test_overlays_qwen_without_changing_transport_or_other_models(self):
        catalog = {
            "models": [
                {
                    "slug": "Qwen/test",
                    "context_window": 42,
                    "base_instructions": "legacy",
                    "model_messages": {
                        "instructions_template": "old",
                        "instructions_variables": {"old": 1},
                        "permissions": {"never": "Keep permissions"},
                    },
                },
                {"slug": "other", "base_instructions": "Leave alone"},
            ]
        }
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "candidate"
            build_bundle(catalog, output)
            result = json.loads((output / "models.json").read_text())
            model = result["models"][0]
            self.assertEqual(model["context_window"], 42)
            self.assertEqual(
                model["model_messages"]["permissions"], {"never": "Keep permissions"}
            )
            self.assertIsNone(model["model_messages"]["instructions_variables"])
            self.assertEqual(
                model["base_instructions"], (output / "shared.md").read_text()
            )
            self.assertEqual(result["models"][1], catalog["models"][1])
            self.assertEqual(catalog["models"][0]["base_instructions"], "legacy")

    def test_profile_uses_shared_once_and_roles_only_as_overrides(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "space in path 🚀"
            build_bundle({"models": [{"slug": "Qwen/test"}]}, output)
            profile = tomllib.loads((output / "profile.toml").read_text())
            self.assertEqual(
                Path(profile["model_instructions_file"]), output.resolve() / "shared.md"
            )
            self.assertNotIn("approval_policy", profile)
            self.assertNotIn("model_providers", profile)
            for name in ("worker", "reviewer"):
                role = tomllib.loads(
                    Path(profile["agents"][name]["config_file"]).read_text()
                )
                self.assertEqual(set(role), {"developer_instructions"})
                self.assertNotIn(
                    (output / "shared.md").read_text(), role["developer_instructions"]
                )

    def test_invalid_catalog_does_not_create_partial_bundle(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "invalid"
            with self.assertRaises(ValueError):
                build_bundle({"models": [{"slug": "other"}]}, output)
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
