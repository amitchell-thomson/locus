"""On-demand llama.cpp server for GPU-side figure descriptions (plan step 11.6).

Ollama ≤0.30.6 runs the qwen2.5vl vision encoder on CPU (~25 s/figure of pure encode);
llama.cpp offloads the mmproj to GPU. This module spawns `llama-server` for the duration
of a figure batch and speaks its OpenAI-compatible chat API — the GOT-OCR choreography
pattern (load → use → release), slotted into the step-11.5 VRAM guards: ALL resident
Ollama models are evicted (confirmed-settle) before the server takes the card.

Failure semantics, in line with §6 quarantine-not-crash:
  - lifecycle failures (binary absent, startup timeout, server died) raise
    `LlamaServerError` — callers fall back to the Ollama engine for the (remaining)
    figures and record a doc gap; a dead server must never quarantine a document;
  - per-call transport/HTTP errors raise `llm.IngestExtractionError` — the exception
    `describe_figure` already degrades on (caption-only figure).
"""

from __future__ import annotations

import base64
import json
import logging
import os
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

from locus.config import FiguresConfig, load
from locus.ingest import llm

log = logging.getLogger(__name__)

_HEALTH_POLL_SECONDS = 2.0
_TERMINATE_GRACE_SECONDS = 10.0
_SERVER_LOG = Path("/tmp/locus-llama-server.log")


class LlamaServerError(RuntimeError):
    """llama-server lifecycle failure (spawn/health/death) — fall back, never quarantine."""


class LlamaServer:
    """Context manager owning one llama-server process for a figure batch."""

    def __init__(self, cfg: FiguresConfig | None = None):
        self.cfg = cfg or load().figures
        self.proc: subprocess.Popen | None = None
        self._base = f"http://{self.cfg.llamacpp_host}:{self.cfg.llamacpp_port}"

    def _command(self) -> list[str]:
        c = self.cfg
        model_arg = (
            ["-m", c.llamacpp_model]
            if c.llamacpp_model.endswith(".gguf")
            else ["-hf", c.llamacpp_model]
        )
        cmd = [
            c.llamacpp_binary, *model_arg,
            "-ngl", str(c.llamacpp_ngl), "-c", str(c.llamacpp_ctx),
            "--host", c.llamacpp_host, "--port", str(c.llamacpp_port),
        ]
        if c.llamacpp_mmproj:
            cmd += ["--mmproj", c.llamacpp_mmproj]
        return cmd

    def __enter__(self) -> "LlamaServer":
        # The card must be empty before the server plans its GPU allocation (step 11.5:
        # loads racing another model's teardown get planned split/OOM).
        llm.unload_all()
        cmd = self._command()
        env = dict(os.environ)
        # Release tarballs ship their shared libs beside the binary; an absolute binary
        # path implies its directory must be on the loader path.
        binary = Path(self.cfg.llamacpp_binary)
        if binary.is_absolute():
            env["LD_LIBRARY_PATH"] = (
                f"{binary.parent}:{env.get('LD_LIBRARY_PATH', '')}".rstrip(":")
            )
        try:
            log_f = _SERVER_LOG.open("ab")
            self.proc = subprocess.Popen(cmd, stdout=log_f, stderr=subprocess.STDOUT, env=env)
        except OSError as exc:  # binary missing / not executable
            raise LlamaServerError(f"could not spawn {cmd[0]!r}: {exc}") from exc

        deadline = time.monotonic() + self.cfg.llamacpp_startup_timeout
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise LlamaServerError(
                    f"llama-server exited during startup (rc={self.proc.returncode}); "
                    f"see {_SERVER_LOG}"
                )
            try:
                with urllib.request.urlopen(f"{self._base}/health", timeout=5) as resp:
                    if json.load(resp).get("status") == "ok":
                        return self
            except (urllib.error.URLError, json.JSONDecodeError, TimeoutError):
                pass
            time.sleep(_HEALTH_POLL_SECONDS)
        self._terminate()
        raise LlamaServerError(
            f"llama-server not healthy within {self.cfg.llamacpp_startup_timeout:.0f}s; "
            f"see {_SERVER_LOG}"
        )

    def __exit__(self, *exc_info) -> None:
        self._terminate()

    def _terminate(self) -> None:
        if self.proc is None or self.proc.poll() is not None:
            return
        self.proc.terminate()
        try:
            self.proc.wait(timeout=_TERMINATE_GRACE_SECONDS)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=_TERMINATE_GRACE_SECONDS)

    def chat(
        self,
        prompt: str,
        image_bytes: bytes,
        *,
        temperature: float = 0.0,
        max_tokens: int = 512,
    ) -> str:
        """One multimodal chat call (OpenAI shape, image as a base64 data URI)."""
        if self.proc is not None and self.proc.poll() is not None:
            # The server died mid-batch: a lifecycle failure, not a per-call blip — the
            # caller should fall back to the Ollama engine for the remaining figures
            # instead of retrying a corpse.
            raise LlamaServerError(
                f"llama-server died mid-batch (rc={self.proc.returncode}); see {_SERVER_LOG}"
            )
        body = json.dumps(
            {
                "messages": [
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": prompt},
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": "data:image/png;base64,"
                                    + base64.standard_b64encode(image_bytes).decode()
                                },
                            },
                        ],
                    }
                ],
                "temperature": temperature,
                "max_tokens": max_tokens,
            }
        ).encode()
        req = urllib.request.Request(
            f"{self._base}/v1/chat/completions",
            data=body,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=300) as resp:
                out = json.load(resp)
            return out["choices"][0]["message"]["content"]
        except Exception as exc:  # transport/HTTP/parse: per-call failure, degrade per-figure
            raise llm.IngestExtractionError(f"llama-server chat failed: {exc}") from exc
