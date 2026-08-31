"""GitHub-backed invitations and short-lived ZIPCODE service sessions."""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import os
import secrets
import sqlite3
import time
from dataclasses import dataclass
from pathlib import Path

import httpx
from fastapi import APIRouter, HTTPException, Request, Response, status
from pydantic import BaseModel


ACCESS_TOKEN_SECONDS = int(os.environ.get("ZIPCODE_ACCESS_TOKEN_SECONDS", "900"))
REFRESH_TOKEN_SECONDS = int(os.environ.get("ZIPCODE_REFRESH_TOKEN_SECONDS", "2592000"))
DATABASE_PATH = Path(os.environ.get("ZIPCODE_AUTH_DATABASE", "/var/lib/zipcode/auth.sqlite3"))
JWT_ISSUER = os.environ.get("ZIPCODE_JWT_ISSUER", "zipcode")
JWT_AUDIENCE = os.environ.get("ZIPCODE_JWT_AUDIENCE", "zipcode-api")


class GithubExchange(BaseModel):
    github_token: str


class RefreshRequest(BaseModel):
    refresh_token: str


class LogoutRequest(BaseModel):
    refresh_token: str


@dataclass(frozen=True)
class Identity:
    login: str


class InvitationStore:
    def __init__(self, path: Path = DATABASE_PATH) -> None:
        self.path = path

    def initialize(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self._connect() as database:
            database.executescript(
                """
                CREATE TABLE IF NOT EXISTS invitations (
                    github_login TEXT PRIMARY KEY COLLATE NOCASE,
                    invited_at INTEGER NOT NULL,
                    expires_at INTEGER,
                    enabled INTEGER NOT NULL DEFAULT 1
                );
                CREATE TABLE IF NOT EXISTS refresh_tokens (
                    token_hash TEXT PRIMARY KEY,
                    github_login TEXT NOT NULL COLLATE NOCASE,
                    issued_at INTEGER NOT NULL,
                    expires_at INTEGER NOT NULL,
                    revoked_at INTEGER
                );
                CREATE INDEX IF NOT EXISTS refresh_tokens_login
                    ON refresh_tokens(github_login);
                """
            )
        self.path.chmod(0o600)

    def invite(self, login: str, expires_at: int | None = None) -> None:
        normalized = normalize_login(login)
        self.initialize()
        with self._connect() as database:
            database.execute(
                """
                INSERT INTO invitations(github_login, invited_at, expires_at, enabled)
                VALUES (?, ?, ?, 1)
                ON CONFLICT(github_login) DO UPDATE SET
                    invited_at = excluded.invited_at,
                    expires_at = excluded.expires_at,
                    enabled = 1
                """,
                (normalized, int(time.time()), expires_at),
            )

    def revoke(self, login: str) -> None:
        normalized = normalize_login(login)
        now = int(time.time())
        self.initialize()
        with self._connect() as database:
            database.execute(
                "UPDATE invitations SET enabled = 0 WHERE github_login = ?",
                (normalized,),
            )
            database.execute(
                """
                UPDATE refresh_tokens SET revoked_at = ?
                WHERE github_login = ? AND revoked_at IS NULL
                """,
                (now, normalized),
            )

    def list_invitations(self) -> list[sqlite3.Row]:
        self.initialize()
        with self._connect() as database:
            return list(
                database.execute(
                    """
                    SELECT github_login, invited_at, expires_at, enabled
                    FROM invitations ORDER BY github_login
                    """
                )
            )

    def is_allowed(self, login: str, now: int | None = None) -> bool:
        normalized = normalize_login(login)
        if normalized in configured_allowlist():
            return True
        self.initialize()
        instant = int(time.time()) if now is None else now
        with self._connect() as database:
            return self._is_invited(database, normalized, instant)

    def create_refresh_token(self, login: str, now: int | None = None) -> str:
        instant = int(time.time()) if now is None else now
        token = secrets.token_urlsafe(48)
        self.initialize()
        with self._connect() as database:
            database.execute(
                """
                INSERT INTO refresh_tokens(
                    token_hash, github_login, issued_at, expires_at, revoked_at
                ) VALUES (?, ?, ?, ?, NULL)
                """,
                (
                    hash_token(token),
                    normalize_login(login),
                    instant,
                    instant + REFRESH_TOKEN_SECONDS,
                ),
            )
        return token

    def rotate_refresh_token(self, token: str, now: int | None = None) -> tuple[str, str] | None:
        instant = int(time.time()) if now is None else now
        token_hash = hash_token(token)
        self.initialize()
        with self._connect() as database:
            database.execute("BEGIN IMMEDIATE")
            row = database.execute(
                """
                SELECT github_login FROM refresh_tokens
                WHERE token_hash = ? AND revoked_at IS NULL AND expires_at > ?
                """,
                (token_hash, instant),
            ).fetchone()
            if row is None:
                return None
            login = normalize_login(row["github_login"])
            if login not in configured_allowlist() and not self._is_invited(
                database, login, instant
            ):
                database.execute(
                    "UPDATE refresh_tokens SET revoked_at = ? WHERE token_hash = ?",
                    (instant, token_hash),
                )
                return None
            replacement = secrets.token_urlsafe(48)
            database.execute(
                "UPDATE refresh_tokens SET revoked_at = ? WHERE token_hash = ?",
                (instant, token_hash),
            )
            database.execute(
                """
                INSERT INTO refresh_tokens(
                    token_hash, github_login, issued_at, expires_at, revoked_at
                ) VALUES (?, ?, ?, ?, NULL)
                """,
                (
                    hash_token(replacement),
                    login,
                    instant,
                    instant + REFRESH_TOKEN_SECONDS,
                ),
            )
            return login, replacement

    def revoke_refresh_token(self, token: str) -> None:
        self.initialize()
        with self._connect() as database:
            database.execute(
                "UPDATE refresh_tokens SET revoked_at = ? WHERE token_hash = ?",
                (int(time.time()), hash_token(token)),
            )

    def _connect(self) -> sqlite3.Connection:
        database = sqlite3.connect(self.path, timeout=10)
        database.row_factory = sqlite3.Row
        return database

    @staticmethod
    def _is_invited(database: sqlite3.Connection, login: str, instant: int) -> bool:
        row = database.execute(
            """
            SELECT 1 FROM invitations
            WHERE github_login = ? AND enabled = 1
              AND (expires_at IS NULL OR expires_at > ?)
            """,
            (normalize_login(login), instant),
        ).fetchone()
        return row is not None


store = InvitationStore()
router = APIRouter(prefix="/v1/auth", tags=["authentication"])


@router.post("/exchange")
async def exchange(body: GithubExchange) -> dict[str, str | int]:
    async with httpx.AsyncClient(timeout=15) as client:
        response = await client.get(
            "https://api.github.com/user",
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {body.github_token}",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": "zipcode-control-plane",
            },
        )
    if response.status_code != 200:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, "GitHub authentication failed")
    payload = response.json()
    login = normalize_login(payload.get("login", ""))
    if not login or not store.is_allowed(login):
        raise HTTPException(status.HTTP_403_FORBIDDEN, "No active ZIPCODE invitation")
    return issue_session(login)


@router.post("/refresh")
async def refresh(body: RefreshRequest) -> dict[str, str | int]:
    rotated = store.rotate_refresh_token(body.refresh_token)
    if rotated is None:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, "Session expired or revoked")
    login, refresh_token = rotated
    return issue_session(login, refresh_token=refresh_token)


@router.post("/logout", status_code=status.HTTP_204_NO_CONTENT, response_class=Response)
async def logout(body: LogoutRequest) -> Response:
    store.revoke_refresh_token(body.refresh_token)
    return Response(status_code=status.HTTP_204_NO_CONTENT)


@router.get("/me")
async def me(request: Request) -> dict[str, str]:
    identity = authenticate_request(request)
    return {"github_login": identity.login}


def authenticate_request(request: Request) -> Identity:
    authorization = request.headers.get("authorization", "")
    scheme, _, token = authorization.partition(" ")
    if scheme.lower() != "bearer" or not token:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, "Missing bearer token")
    try:
        claims = decode_access_token(token)
    except ValueError as error:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, str(error)) from error
    login = normalize_login(str(claims.get("sub", "")))
    if not login or not store.is_allowed(login):
        raise HTTPException(status.HTTP_403_FORBIDDEN, "ZIPCODE invitation revoked")
    return Identity(login=login)


def issue_session(login: str, refresh_token: str | None = None) -> dict[str, str | int]:
    return {
        "access_token": encode_access_token(login),
        "refresh_token": refresh_token or store.create_refresh_token(login),
        "token_type": "Bearer",
        "expires_in": ACCESS_TOKEN_SECONDS,
        "github_login": normalize_login(login),
    }


def encode_access_token(login: str, now: int | None = None) -> str:
    instant = int(time.time()) if now is None else now
    header = {"alg": "HS256", "typ": "JWT"}
    claims = {
        "iss": JWT_ISSUER,
        "aud": JWT_AUDIENCE,
        "sub": normalize_login(login),
        "iat": instant,
        "exp": instant + ACCESS_TOKEN_SECONDS,
        "jti": secrets.token_hex(16),
    }
    unsigned = f"{b64_json(header)}.{b64_json(claims)}"
    signature = hmac.new(jwt_secret(), unsigned.encode(), hashlib.sha256).digest()
    return f"{unsigned}.{b64_encode(signature)}"


def decode_access_token(token: str, now: int | None = None) -> dict[str, object]:
    try:
        header_segment, claims_segment, signature_segment = token.split(".")
        unsigned = f"{header_segment}.{claims_segment}"
        expected = hmac.new(jwt_secret(), unsigned.encode(), hashlib.sha256).digest()
        supplied = b64_decode(signature_segment)
        if not hmac.compare_digest(expected, supplied):
            raise ValueError("Invalid access token")
        header = json.loads(b64_decode(header_segment))
        claims = json.loads(b64_decode(claims_segment))
    except (ValueError, TypeError, json.JSONDecodeError) as error:
        raise ValueError("Invalid access token") from error
    instant = int(time.time()) if now is None else now
    if header != {"alg": "HS256", "typ": "JWT"}:
        raise ValueError("Invalid access token")
    if claims.get("iss") != JWT_ISSUER or claims.get("aud") != JWT_AUDIENCE:
        raise ValueError("Invalid access token")
    if not isinstance(claims.get("exp"), int) or claims["exp"] <= instant:
        raise ValueError("Access token expired")
    return claims


def jwt_secret() -> bytes:
    value = os.environ.get("ZIPCODE_JWT_SECRET", "")
    if len(value.encode()) < 32:
        raise RuntimeError("ZIPCODE_JWT_SECRET must contain at least 32 bytes")
    return value.encode()


def normalize_login(login: str) -> str:
    return login.strip().lower()


def configured_allowlist() -> set[str]:
    return {
        normalize_login(item)
        for item in os.environ.get("ZIPCODE_GITHUB_ALLOWLIST", "").split(",")
        if item.strip()
    }


def hash_token(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


def b64_json(value: object) -> str:
    return b64_encode(json.dumps(value, separators=(",", ":"), sort_keys=True).encode())


def b64_encode(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode()


def b64_decode(value: str) -> bytes:
    decoded = base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))
    if b64_encode(decoded) != value:
        raise ValueError("Invalid base64url encoding")
    return decoded
