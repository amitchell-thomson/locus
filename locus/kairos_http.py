"""kairosd's HTTP API from Python: the token and URL from kairos's own config, one call.

Used by the Canvas deadlines (spec P6) and the learning goals' tasks (spec L3). Every write is a
changeset kairosd assesses for risk itself (risk is computed, never supplied); `author` says who
asked, never how risky it is.
"""

from __future__ import annotations

import json
import subprocess
import tomllib
import urllib.request
from pathlib import Path


def kairos() -> tuple[str, str]:
    cfg = tomllib.loads(Path("~/.config/kairos/config.toml").expanduser().read_text())["daemon"]
    tok = subprocess.run(cfg["token_cmd"], shell=True, capture_output=True, text=True, check=True).stdout.strip()
    return cfg["url"].rstrip("/"), tok


def call(method: str, path: str, body: dict | None = None, headers: dict | None = None, *,
         author: str = "claude"):
    url, tok = kairos()
    req = urllib.request.Request(url + path, method=method,
                                 data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json",
                                          "x-kairos-author": author, **(headers or {})})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read() or b"null")
