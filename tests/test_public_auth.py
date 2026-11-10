"""The public listener's OAuth (H1.10): the whole flow claude.ai runs, and what it must refuse."""

from __future__ import annotations

import base64
import hashlib
import secrets
from urllib.parse import parse_qs, urlparse

import pytest
from starlette.testclient import TestClient

from locus import mcp_server
from locus.public_auth import CLAUDE_CALLBACK, Provider, Store, redirect_allowed

BASE = "https://compute-node.example.ts.net:8443"
PHRASE = "correct horse battery staple"


@pytest.fixture
def client(tmp_path, monkeypatch):
    monkeypatch.setattr(mcp_server.mcp_log, "record", lambda *a, **k: None)
    provider = Provider(Store(tmp_path / "auth.db"), PHRASE, BASE)
    app = mcp_server._build(host="127.0.0.1", public=(provider, BASE)).streamable_http_app()
    with TestClient(app, base_url=BASE) as c:
        yield c


def _register(c, redirect=CLAUDE_CALLBACK):
    return c.post("/register", json={"redirect_uris": [redirect], "client_name": "claude",
                                     "token_endpoint_auth_method": "none",
                                     "grant_types": ["authorization_code", "refresh_token"],
                                     "response_types": ["code"]})


def _pkce():
    v = secrets.token_urlsafe(48)
    ch = base64.urlsafe_b64encode(hashlib.sha256(v.encode()).digest()).rstrip(b"=").decode()
    return v, ch


def _authorize(c, cid, challenge):
    r = c.get("/authorize", params={"response_type": "code", "client_id": cid, "redirect_uri": CLAUDE_CALLBACK,
                                     "code_challenge": challenge, "code_challenge_method": "S256", "state": "s1"},
              follow_redirects=False)
    assert r.status_code in (302, 307), r.text
    loc = r.headers["location"]
    assert loc.startswith(f"{BASE}/login?req=")
    return parse_qs(urlparse(loc).query)["req"][0]


def test_the_whole_flow_ends_in_a_working_token(client):
    cid = _register(client).json()["client_id"]
    verifier, challenge = _pkce()
    rid = _authorize(client, cid, challenge)
    page = client.get("/login", params={"req": rid})
    assert "passphrase" in page.text
    r = client.post("/login", data={"req": rid, "passphrase": PHRASE}, follow_redirects=False)
    assert r.status_code == 302 and r.headers["location"].startswith(CLAUDE_CALLBACK)
    q = parse_qs(urlparse(r.headers["location"]).query)
    assert q["state"] == ["s1"]
    tok = client.post("/token", data={"grant_type": "authorization_code", "code": q["code"][0], "client_id": cid,
                                      "redirect_uri": CLAUDE_CALLBACK, "code_verifier": verifier}).json()
    assert tok["token_type"].lower() == "bearer" and tok["refresh_token"]
    init = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}
    hdrs = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json"}
    assert client.post("/mcp", json=init, headers=hdrs).status_code == 401
    ok = client.post("/mcp", json=init, headers={**hdrs, "Authorization": f"Bearer {tok['access_token']}"})
    assert ok.status_code == 200 and "locus" in ok.text
    # a code works once
    again = client.post("/token", data={"grant_type": "authorization_code", "code": q["code"][0], "client_id": cid,
                                        "redirect_uri": CLAUDE_CALLBACK, "code_verifier": verifier})
    assert again.status_code == 400


def test_a_wrong_passphrase_issues_nothing_and_five_lock_the_page(client):
    cid = _register(client).json()["client_id"]
    rid = _authorize(client, cid, _pkce()[1])
    for _ in range(5):
        r = client.post("/login", data={"req": rid, "passphrase": "guess"}, follow_redirects=False)
        assert r.status_code == 401
    r = client.post("/login", data={"req": rid, "passphrase": PHRASE}, follow_redirects=False)
    assert r.status_code == 401 and "Too many" in r.text


def test_a_client_cannot_register_a_redirect_to_somewhere_else(client):
    assert _register(client, "https://evil.example/cb").status_code == 400


def test_redirects_allowed():
    assert redirect_allowed(CLAUDE_CALLBACK)
    assert redirect_allowed("http://localhost:3118/callback") and redirect_allowed("http://127.0.0.1:9/cb")
    assert not redirect_allowed("https://claude.ai.evil.com/api/mcp/auth_callback")
    assert not redirect_allowed("https://localhost/cb")


def test_a_short_passphrase_is_refused(tmp_path):
    with pytest.raises(ValueError):
        Provider(Store(tmp_path / "a.db"), "short", BASE)
