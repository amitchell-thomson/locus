"""The single-user OAuth login in front of the public endpoint (H1.10, D4).

claude.ai and the phone app reach MCP servers from Anthropic's cloud, so the service has to be on
public HTTPS (Tailscale Funnel), and claude.ai connectors take OAuth: a fixed header token is
"Beta, for a limited set of organizations" (H1.1, Anthropic's connector docs, 10 Nov). This is
the authorization server for exactly one person:

* **Dynamic Client Registration** (RFC 7591), which claude.ai uses by default. A client may only
  register redirect URIs on the allow list below (claude.ai's callback and Claude Code's
  loopback), so a stranger who registers a client cannot have a code sent anywhere else.
* **The approval page** asks for a passphrase that only he knows (Keychain
  `locus-public-passphrase` on the laptop, `LOCUS_PUBLIC_PASSPHRASE` in the server's env file).
  Five wrong answers in 15 minutes lock the page for 15 minutes.
* **Tokens** are random, kept only as SHA-256 hashes in `data/public-auth.db`, so a copy of that
  file grants nothing. Access tokens last a day and refresh tokens 90 days; claude.ai refreshes
  by itself, so he signs in once.

The tailnet listener (`locus-http`, bearer token) is unchanged and does not accept these tokens;
this listener accepts nothing else.
"""

from __future__ import annotations

import hashlib
import hmac
import html
import json
import secrets
import sqlite3
import time
from pathlib import Path
from urllib.parse import urlparse

from mcp.server.auth.provider import (
    AccessToken,
    AuthorizationCode,
    AuthorizationParams,
    AuthorizeError,
    RefreshToken,
    RegistrationError,
    TokenError,
    construct_redirect_uri,
)
from mcp.shared.auth import OAuthClientInformationFull, OAuthToken

ACCESS_TTL = 24 * 3600
REFRESH_TTL = 90 * 24 * 3600
CODE_TTL = 300
PENDING_TTL = 600
LOCKOUT_FAILS, LOCKOUT_WINDOW = 5, 15 * 60
CLAUDE_CALLBACK = "https://claude.ai/api/mcp/auth_callback"


def redirect_allowed(uri: str) -> bool:
    """claude.ai's callback, or a loopback redirect on any port (Claude Code)."""
    if uri == CLAUDE_CALLBACK:
        return True
    u = urlparse(uri)
    return u.scheme == "http" and u.hostname in ("localhost", "127.0.0.1")


def _h(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


class Store:
    def __init__(self, path: Path | str):
        self.conn = sqlite3.connect(str(path), check_same_thread=False)
        self.conn.executescript("""
            CREATE TABLE IF NOT EXISTS clients (id TEXT PRIMARY KEY, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS grants (
                hash TEXT PRIMARY KEY, kind TEXT NOT NULL, body TEXT NOT NULL, expires_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS pending (id TEXT PRIMARY KEY, body TEXT NOT NULL, expires_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS failures (at INTEGER NOT NULL);
        """)

    def put(self, table: str, key: str, body: str, expires_at: int | None = None) -> None:
        with self.conn:
            if table == "clients":
                self.conn.execute("INSERT OR REPLACE INTO clients VALUES (?,?)", (key, body))
            elif table == "pending":
                self.conn.execute("INSERT OR REPLACE INTO pending VALUES (?,?,?)", (key, body, expires_at))
            else:
                self.conn.execute("INSERT OR REPLACE INTO grants VALUES (?,?,?,?)", (key, table, body, expires_at))

    def get(self, table: str, key: str) -> str | None:
        now = int(time.time())
        if table == "clients":
            r = self.conn.execute("SELECT body FROM clients WHERE id=?", (key,)).fetchone()
        elif table == "pending":
            r = self.conn.execute("SELECT body FROM pending WHERE id=? AND expires_at>?", (key, now)).fetchone()
        else:
            r = self.conn.execute("SELECT body FROM grants WHERE hash=? AND kind=? AND expires_at>?",
                                  (key, table, now)).fetchone()
        return r[0] if r else None

    def drop(self, table: str, key: str) -> None:
        with self.conn:
            if table == "pending":
                self.conn.execute("DELETE FROM pending WHERE id=?", (key,))
            else:
                self.conn.execute("DELETE FROM grants WHERE hash=? AND kind=?", (key, table))

    def fail(self) -> None:
        with self.conn:
            self.conn.execute("INSERT INTO failures VALUES (?)", (int(time.time()),))

    def locked(self) -> bool:
        since = int(time.time()) - LOCKOUT_WINDOW
        return self.conn.execute("SELECT COUNT(*) FROM failures WHERE at>?", (since,)).fetchone()[0] >= LOCKOUT_FAILS


class Provider:
    """`OAuthAuthorizationServerProvider` for one person, persisted in SQLite."""

    def __init__(self, store: Store, passphrase: str, base_url: str):
        if len(passphrase) < 12:
            raise ValueError("the public passphrase must be at least 12 characters")
        self.store, self.passphrase, self.base = store, passphrase, base_url.rstrip("/")

    # -- clients
    async def get_client(self, client_id: str) -> OAuthClientInformationFull | None:
        body = self.store.get("clients", client_id)
        return OAuthClientInformationFull.model_validate_json(body) if body else None

    async def register_client(self, client_info: OAuthClientInformationFull) -> None:
        bad = [str(u) for u in client_info.redirect_uris or [] if not redirect_allowed(str(u))]
        if bad or not client_info.redirect_uris:
            raise RegistrationError(error="invalid_redirect_uri",
                                    error_description=f"redirect not allowed: {', '.join(bad) or 'none given'}")
        self.store.put("clients", client_info.client_id, client_info.model_dump_json())

    # -- authorization: park the request, send the browser to the passphrase page
    async def authorize(self, client: OAuthClientInformationFull, params: AuthorizationParams) -> str:
        if not redirect_allowed(str(params.redirect_uri)):
            raise AuthorizeError(error="invalid_request", error_description="redirect not allowed")
        rid = secrets.token_urlsafe(24)
        body = json.dumps({"client_id": client.client_id, "params": params.model_dump(mode="json")})
        self.store.put("pending", rid, body, int(time.time()) + PENDING_TTL)
        return f"{self.base}/login?req={rid}"

    def approve(self, rid: str, passphrase: str) -> tuple[str | None, str]:
        """(redirect URL, message). None means show the page again with the message."""
        if self.store.locked():
            return None, "Too many wrong passphrases. Try again in 15 minutes."
        body = self.store.get("pending", rid)
        if not body:
            return None, "This sign-in request has expired. Start again from claude.ai."
        if not hmac.compare_digest(passphrase.encode(), self.passphrase.encode()):
            self.store.fail()
            return None, "That is not the passphrase."
        self.store.drop("pending", rid)
        req = json.loads(body)
        p = AuthorizationParams.model_validate(req["params"])
        code = secrets.token_urlsafe(32)
        ac = AuthorizationCode(code=code, scopes=p.scopes or [], expires_at=time.time() + CODE_TTL,
                               client_id=req["client_id"], code_challenge=p.code_challenge,
                               redirect_uri=p.redirect_uri, redirect_uri_provided_explicitly=p.redirect_uri_provided_explicitly,
                               resource=p.resource)
        self.store.put("code", _h(code), ac.model_dump_json(), int(time.time()) + CODE_TTL)
        return construct_redirect_uri(str(p.redirect_uri), code=code, state=p.state), "ok"

    async def load_authorization_code(self, client, authorization_code: str) -> AuthorizationCode | None:
        body = self.store.get("code", _h(authorization_code))
        if not body:
            return None
        ac = AuthorizationCode.model_validate_json(body)
        return ac if ac.client_id == client.client_id else None

    def _issue(self, client_id: str, scopes: list[str], resource: str | None) -> OAuthToken:
        access, refresh = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
        now = int(time.time())
        self.store.put("access", _h(access), AccessToken(token=_h(access), client_id=client_id, scopes=scopes,
                       expires_at=now + ACCESS_TTL, resource=resource).model_dump_json(), now + ACCESS_TTL)
        self.store.put("refresh", _h(refresh), RefreshToken(token=_h(refresh), client_id=client_id, scopes=scopes,
                       expires_at=now + REFRESH_TTL).model_dump_json(), now + REFRESH_TTL)
        return OAuthToken(access_token=access, token_type="Bearer", expires_in=ACCESS_TTL,
                          scope=" ".join(scopes) or None, refresh_token=refresh)

    async def exchange_authorization_code(self, client, authorization_code: AuthorizationCode) -> OAuthToken:
        self.store.drop("code", _h(authorization_code.code))  # one use
        return self._issue(client.client_id, authorization_code.scopes, authorization_code.resource)

    async def load_refresh_token(self, client, refresh_token: str) -> RefreshToken | None:
        body = self.store.get("refresh", _h(refresh_token))
        if not body:
            return None
        rt = RefreshToken.model_validate_json(body)
        return rt if rt.client_id == client.client_id else None

    async def exchange_refresh_token(self, client, refresh_token: RefreshToken, scopes: list[str]) -> OAuthToken:
        # The stored token field is already the hash; rotate it.
        self.store.drop("refresh", refresh_token.token)
        if scopes and not set(scopes) <= set(refresh_token.scopes):
            raise TokenError(error="invalid_scope")
        return self._issue(client.client_id, scopes or refresh_token.scopes, None)

    async def load_access_token(self, token: str) -> AccessToken | None:
        body = self.store.get("access", _h(token))
        return AccessToken.model_validate_json(body) if body else None

    async def revoke_token(self, token) -> None:
        kind = "refresh" if isinstance(token, RefreshToken) else "access"
        self.store.drop(kind, token.token)


def login_page(rid: str, message: str = "") -> str:
    msg = f'<p class="m">{html.escape(message)}</p>' if message else ""
    return f"""<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>locus sign-in</title><style>
body{{font:16px -apple-system,system-ui,sans-serif;background:#1e1e2e;color:#cdd6f4;display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0;padding:16px}}
form{{max-width:340px;width:100%}}h1{{font-size:20px;font-weight:600}}p{{color:#a6adc8}}.m{{color:#f38ba8}}
input,button{{width:100%;box-sizing:border-box;font-size:16px;padding:12px;border-radius:8px;border:1px solid #45475a;margin-top:8px}}
input{{background:#313244;color:#cdd6f4}}button{{background:#b4befe;color:#1e1e2e;border:0;font-weight:600}}
</style></head><body><form method="post" action="login"><h1>locus</h1>
<p>Claude is asking to read your locus: mail, calendar, chats, repos and library.</p>{msg}
<input type="hidden" name="req" value="{html.escape(rid)}">
<input type="password" name="passphrase" placeholder="passphrase" autocomplete="current-password" autofocus>
<button type="submit">Allow</button></form></body></html>"""
