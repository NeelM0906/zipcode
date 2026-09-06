from __future__ import annotations

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
        with tempfile.TemporaryDirectory() as temporary:
            installer_path = Path(temporary) / "install.sh"
            installer_path.write_bytes(installer)
            with patch.object(model_mux, "SETUP_SCRIPT_PATH", str(installer_path)):
                with TestClient(model_mux.app) as client:
                    canonical = client.get("/install.sh")
                    legacy = client.get("/install/zip-code-setup.sh")
                    checksum = client.get("/install.sha256")

        self.assertEqual(canonical.status_code, 200)
        self.assertEqual(canonical.content, installer)
        self.assertEqual(
            canonical.headers["content-disposition"],
            'attachment; filename="install.sh"',
        )
        self.assertEqual(legacy.content, canonical.content)
        self.assertEqual(
            checksum.text,
            f"{hashlib.sha256(installer).hexdigest()}  install.sh\n",
        )


if __name__ == "__main__":
    unittest.main()
