import hashlib
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from fastapi.testclient import TestClient

from gateway import model_mux


class InstallerRouteTests(unittest.TestCase):
    def test_installer_and_legacy_alias_serve_release_installer(self) -> None:
        installer = b"#!/usr/bin/env sh\necho ZIPCODE\n"
        routes = (
            ("/install.sh", "/install.sha256", "install.sh"),
            (
                "/install/zip-code-setup.sh",
                "/install/zip-code-setup.sha256",
                "zip-code-setup.sh",
            ),
            (
                "/install/qwen-codex-setup.sh",
                "/install/qwen-codex-setup.sha256",
                "qwen-codex-setup.sh",
            ),
        )
        with tempfile.TemporaryDirectory() as temporary:
            installer_path = Path(temporary) / "install.sh"
            installer_path.write_bytes(installer)
            with patch.object(model_mux, "SETUP_SCRIPT_PATH", str(installer_path)):
                with TestClient(model_mux.app) as client:
                    for script_route, checksum_route, filename in routes:
                        with self.subTest(script_route=script_route):
                            response = client.get(script_route)
                            checksum = client.get(checksum_route)

                            self.assertEqual(response.status_code, 200)
                            self.assertEqual(response.content, installer)
                            self.assertEqual(
                                response.headers["content-disposition"],
                                f'attachment; filename="{filename}"',
                            )
                            self.assertEqual(
                                checksum.text,
                                f"{hashlib.sha256(installer).hexdigest()}  {filename}\n",
                            )


if __name__ == "__main__":
    unittest.main()
