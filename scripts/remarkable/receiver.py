#!/usr/bin/env python3
"""Durable receiver for reMarkable device-rendered PDFs (capture transport endpoint).

The on-device Locus agent (persistent systemd timer on the tablet) renders every
changed document with xochitl's own renderer and pushes it over the tailnet as:

    b"LOCUSDOC <uuid> <metadata-md5> <size>\n" + <pdf bytes>

This process listens on the server's tailnet IP and writes each render to a durable
staging directory as <uuid>.pdf (overwriting on change, so it's always the latest
render, one file per doc). It is intentionally a *staging* landing zone: the
folder->category mapping + maturity=rough tagging + hash-idempotent `locus ingest`
wiring is separate server-side work (Loop A) that reads from here.

Run via the `locus-remarkable-receiver` systemd --user service (see the unit next to
this file). Binds the tailnet IP so it is never exposed on the LAN/public interfaces.
"""
from __future__ import annotations

import os
import socket
import sys
from datetime import datetime, timezone

TAILNET_IP = os.environ.get("LOCUS_RM_BIND", "100.117.10.28")
PORT = int(os.environ.get("LOCUS_RM_PORT", "9010"))
STAGE = os.environ.get("LOCUS_RM_STAGE", "/home/alec/remarkable-import")
IDLE_CLOSE = 6  # seconds of silence after last byte -> assume transfer done


def log(msg: str) -> None:
    print(f"{datetime.now(timezone.utc).isoformat(timespec='seconds')} {msg}", flush=True)


def main() -> int:
    os.makedirs(STAGE, exist_ok=True)
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        s.bind((TAILNET_IP, PORT))
    except OSError as e:
        # port busy (e.g. another receiver still holding it) -> exit non-zero so
        # systemd Restart=always retries until it frees.
        log(f"bind {TAILNET_IP}:{PORT} failed: {e}")
        return 1
    s.listen(8)
    log(f"listening {TAILNET_IP}:{PORT} -> {STAGE}")
    while True:
        conn, addr = s.accept()
        conn.settimeout(IDLE_CLOSE)
        data = b""
        while True:
            try:
                chunk = conn.recv(65536)
            except socket.timeout:
                break
            except OSError:
                break
            if not chunk:
                break
            data += chunk
        conn.close()
        if data.startswith(b"LOCUSDOC "):
            header, _, body = data.partition(b"\n")
            parts = header.decode("utf-8", "replace").split()
            uuid = parts[1] if len(parts) > 1 else f"unknown-{int(datetime.now().timestamp())}"
            valid = body[:4] == b"%PDF"
            tmp = os.path.join(STAGE, f".{uuid}.pdf.part")
            with open(tmp, "wb") as fh:
                fh.write(body)
            os.replace(tmp, os.path.join(STAGE, f"{uuid}.pdf"))  # atomic overwrite
            log(f"doc {uuid} {len(body)}B pdf={valid} from {addr[0]}")
        else:
            log(f"ignored non-LOCUSDOC payload {len(data)}B from {addr[0]}")


if __name__ == "__main__":
    sys.exit(main())
