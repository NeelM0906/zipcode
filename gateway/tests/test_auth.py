from __future__ import annotations

import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from fastapi import HTTPException
from starlette.requests import Request

from gateway.auth import (
    InvitationStore,
    authenticate_request,
    decode_access_token,
    encode_access_token,
)


class AuthenticationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.environment = patch.dict(
            os.environ,
            {
                "ZIPCODE_GITHUB_ALLOWLIST": "",
                "ZIPCODE_JWT_SECRET": "x" * 32,
            },
        )
        self.environment.start()
        self.tempdir = tempfile.TemporaryDirectory()
        self.store = InvitationStore(Path(self.tempdir.name) / "auth.sqlite3")

    def tearDown(self) -> None:
        self.tempdir.cleanup()
        self.environment.stop()

    def test_invite_refresh_rotation_and_revoke(self) -> None:
        self.store.invite("NeelM0906")
        self.assertEqual(stat.S_IMODE(self.store.path.stat().st_mode), 0o600)
        self.assertTrue(self.store.is_allowed("neelm0906"))
        refresh = self.store.create_refresh_token("NeelM0906", now=100)
        rotated = self.store.rotate_refresh_token(refresh, now=101)
        self.assertIsNotNone(rotated)
        assert rotated is not None
        self.assertEqual(rotated[0], "neelm0906")
        self.assertIsNone(self.store.rotate_refresh_token(refresh, now=102))
        self.store.revoke("NeelM0906")
        self.assertFalse(self.store.is_allowed("neelm0906"))
        self.assertIsNone(self.store.rotate_refresh_token(rotated[1], now=103))

    def test_access_token_signature_and_expiry(self) -> None:
        token = encode_access_token("NeelM0906", now=100)
        self.assertEqual(decode_access_token(token, now=101)["sub"], "neelm0906")
        with self.assertRaisesRegex(ValueError, "expired"):
            decode_access_token(token, now=100 + 901)

        header, claims, signature = token.split(".")
        forged_signature = ("A" if signature[0] != "A" else "B") + signature[1:]
        with self.assertRaisesRegex(ValueError, "Invalid"):
            decode_access_token(f"{header}.{claims}.{forged_signature}", now=101)

        alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
        final_index = alphabet.index(signature[-1])
        noncanonical_signature = signature[:-1] + alphabet[final_index + 1]
        with self.assertRaisesRegex(ValueError, "Invalid"):
            decode_access_token(f"{header}.{claims}.{noncanonical_signature}", now=101)

    def test_request_authentication_tracks_invitation_revocation(self) -> None:
        self.store.invite("NeelM0906")
        token = encode_access_token("NeelM0906")
        request = Request(
            {
                "type": "http",
                "method": "GET",
                "path": "/v1/models",
                "headers": [(b"authorization", f"Bearer {token}".encode())],
            }
        )
        with patch("gateway.auth.store", self.store):
            self.assertEqual(authenticate_request(request).login, "neelm0906")
            self.store.revoke("NeelM0906")
            with self.assertRaises(HTTPException) as raised:
                authenticate_request(request)
        self.assertEqual(raised.exception.status_code, 403)


if __name__ == "__main__":
    unittest.main()
