"""Step 11.6: the llama.cpp figure engine — lifecycle, request shape, dispatch, fallback.

All model-free: a fake Popen + fake HTTP drive LlamaServer; the pipeline fallback test
monkeypatches the server to fail. The live server (spawn, GPU offload, real chat) is
validated separately by scripts/judge_figures.py and the smoke run.
"""

import json
import types
from io import BytesIO

import pytest

from locus.config import FiguresConfig
from locus.ingest import llamacpp
from locus.ingest.llamacpp import LlamaServer, LlamaServerError
from locus.ingest.llm import IngestExtractionError

CFG = FiguresConfig(
    engine="llamacpp", llamacpp_binary="/fake/llama-server",
    llamacpp_startup_timeout=2.0,
)


class FakeProc:
    def __init__(self, alive: bool = True):
        self._alive = alive
        self.returncode = None if alive else 1
        self.terminated = False

    def poll(self):
        return None if self._alive else self.returncode

    def terminate(self):
        self.terminated = True
        self._alive = False
        self.returncode = 0

    def wait(self, timeout=None):
        return self.returncode

    def kill(self):
        self._alive = False
        self.returncode = -9


def _fake_urlopen(payloads: dict):
    """urlopen fake serving /health and /v1/chat/completions; records request bodies."""
    seen = []

    class _Resp:
        def __init__(self, data):
            self._data = json.dumps(data).encode()

        def read(self):
            return self._data

        def __enter__(self):
            return BytesIO(self._data)

        def __exit__(self, *a):
            return False

    def urlopen(req, timeout=None):
        url = req if isinstance(req, str) else req.full_url
        if url.endswith("/health"):
            return _Resp(payloads["health"])
        if url.endswith("/v1/chat/completions"):
            seen.append(json.loads(req.data))
            return _Resp(payloads["chat"])
        raise AssertionError(f"unexpected url {url}")

    return urlopen, seen


def _entered_server(monkeypatch, *, alive=True) -> tuple[LlamaServer, list]:
    proc = FakeProc(alive=alive)
    monkeypatch.setattr(llamacpp.subprocess, "Popen", lambda *a, **k: proc)
    urlopen, seen = _fake_urlopen(
        {"health": {"status": "ok"},
         "chat": {"choices": [{"message": {"content": "A clear block diagram."}}]}}
    )
    monkeypatch.setattr(llamacpp.urllib.request, "urlopen", urlopen)
    monkeypatch.setattr(llamacpp.llm, "unload_all", lambda *a, **k: 0)
    server = LlamaServer(CFG).__enter__()
    return server, seen


def test_enter_polls_health_and_exit_terminates(monkeypatch):
    server, _ = _entered_server(monkeypatch)
    assert server.proc is not None
    server.__exit__(None, None, None)
    assert server.proc.terminated


def test_startup_timeout_raises_and_cleans_up(monkeypatch):
    proc = FakeProc(alive=True)
    monkeypatch.setattr(llamacpp.subprocess, "Popen", lambda *a, **k: proc)

    def never_healthy(req, timeout=None):
        raise llamacpp.urllib.error.URLError("conn refused")

    monkeypatch.setattr(llamacpp.urllib.request, "urlopen", never_healthy)
    monkeypatch.setattr(llamacpp.llm, "unload_all", lambda *a, **k: 0)
    monkeypatch.setattr(llamacpp, "_HEALTH_POLL_SECONDS", 0.01)
    with pytest.raises(LlamaServerError, match="not healthy"):
        LlamaServer(CFG).__enter__()
    assert proc.terminated


def test_spawn_failure_raises(monkeypatch):
    def boom(*a, **k):
        raise OSError("No such file or directory")

    monkeypatch.setattr(llamacpp.subprocess, "Popen", boom)
    monkeypatch.setattr(llamacpp.llm, "unload_all", lambda *a, **k: 0)
    with pytest.raises(LlamaServerError, match="could not spawn"):
        LlamaServer(CFG).__enter__()


def test_early_exit_during_startup_raises(monkeypatch):
    monkeypatch.setattr(llamacpp.subprocess, "Popen", lambda *a, **k: FakeProc(alive=False))
    monkeypatch.setattr(llamacpp.llm, "unload_all", lambda *a, **k: 0)
    with pytest.raises(LlamaServerError, match="exited during startup"):
        LlamaServer(CFG).__enter__()


def test_chat_builds_openai_data_uri_request(monkeypatch):
    server, seen = _entered_server(monkeypatch)
    out = server.chat("Describe this.", b"PNGBYTES", temperature=0.3, max_tokens=512)
    assert out == "A clear block diagram."
    body = seen[0]
    assert body["temperature"] == 0.3 and body["max_tokens"] == 512
    parts = body["messages"][0]["content"]
    assert parts[0] == {"type": "text", "text": "Describe this."}
    assert parts[1]["type"] == "image_url"
    assert parts[1]["image_url"]["url"].startswith("data:image/png;base64,UE5HQllURVM")


def test_chat_on_dead_server_raises_lifecycle_error(monkeypatch):
    server, _ = _entered_server(monkeypatch)
    server.proc._alive = False
    server.proc.returncode = 137
    with pytest.raises(LlamaServerError, match="died mid-batch"):
        server.chat("p", b"img")


def test_chat_transport_error_is_percall(monkeypatch):
    server, _ = _entered_server(monkeypatch)

    def fail(req, timeout=None):
        raise llamacpp.urllib.error.URLError("boom")

    monkeypatch.setattr(llamacpp.urllib.request, "urlopen", fail)
    with pytest.raises(IngestExtractionError, match="chat failed"):
        server.chat("p", b"img")


def test_describe_figure_routes_llama_client(monkeypatch):
    """A .proc-bearing client goes through .chat with the same QC as the Ollama path."""
    from locus.ingest.figures import describe_figure

    class FakeLlama:
        proc = None

        def __init__(self):
            self.calls = []

        def chat(self, prompt, image_bytes, *, temperature, max_tokens):
            self.calls.append(prompt)
            return "A closed-loop diagram with controller, plant and a feedback path."

    fake = FakeLlama()
    out = describe_figure(b"png", "Figure 1", client=fake, model="unused")
    assert out.startswith("A closed-loop diagram")
    assert len(fake.calls) == 1 and "Figure 1" in fake.calls[0]


def test_describe_figure_llama_qc_retry_then_caption_only(monkeypatch):
    from locus.ingest.figures import describe_figure

    class ShortLlama:
        proc = None
        n = 0

        def chat(self, prompt, image_bytes, *, temperature, max_tokens):
            self.n += 1
            return "Too short."  # fails QC both attempts

    fake = ShortLlama()
    assert describe_figure(b"png", None, client=fake, model="unused") is None
    assert fake.n == 2  # bounded retry preserved across engines


def test_pipeline_falls_back_to_ollama_with_gap(monkeypatch):
    """LlamaServer failing to start must not lose figures: ollama path + gap line."""
    from locus import ingest_pipeline
    from locus.extract.base import ExtractedFigure

    cfg = FiguresConfig(engine="llamacpp")
    fake_cfg = types.SimpleNamespace(figures=cfg)
    monkeypatch.setattr(ingest_pipeline, "load", lambda *a, **k: fake_cfg)
    monkeypatch.setattr(ingest_pipeline.llm, "unload", lambda *a, **k: True)

    class FailingServer:
        def __init__(self, *a, **k):
            pass

        def __enter__(self):
            raise LlamaServerError("binary missing")

        def __exit__(self, *a):
            return False

    import locus.ingest.llamacpp as llamacpp_mod

    monkeypatch.setattr(llamacpp_mod, "LlamaServer", FailingServer)
    monkeypatch.setattr(
        ingest_pipeline.figures_pass, "describe_figure",
        lambda img, cap, client=None: "An ollama-made description of the figure here.",
    )
    figs = [ExtractedFigure(page=1, image_bytes=b"x", kind="raster")]
    gaps: list[str] = []
    out = ingest_pipeline._describe_figures(figs, None, gaps)
    assert out == ["An ollama-made description of the figure here."]
    assert any("fell back to ollama" in g for g in gaps)


def test_pipeline_mid_batch_death_falls_back_for_remaining(monkeypatch):
    from locus import ingest_pipeline
    from locus.extract.base import ExtractedFigure

    cfg = FiguresConfig(engine="llamacpp")
    monkeypatch.setattr(
        ingest_pipeline, "load", lambda *a, **k: types.SimpleNamespace(figures=cfg)
    )
    monkeypatch.setattr(ingest_pipeline.llm, "unload", lambda *a, **k: True)

    class DyingServer:
        proc = None

        def __init__(self, *a, **k):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    import locus.ingest.llamacpp as llamacpp_mod

    monkeypatch.setattr(llamacpp_mod, "LlamaServer", DyingServer)
    calls = {"n": 0}

    def describe(img, cap, client=None):
        calls["n"] += 1
        if client is not None and calls["n"] == 2:
            raise LlamaServerError("died mid-batch")  # second figure kills the server
        return f"made by {'llama' if client is not None else 'ollama'} for {img.decode()}"

    monkeypatch.setattr(ingest_pipeline.figures_pass, "describe_figure", describe)
    figs = [ExtractedFigure(page=i, image_bytes=f"f{i}".encode(), kind="raster") for i in (1, 2, 3)]
    gaps: list[str] = []
    out = ingest_pipeline._describe_figures(figs, None, gaps)
    assert out[0] == "made by llama for f1"
    assert out[1] == "made by ollama for f2"  # the figure the death interrupted, redone
    assert out[2] == "made by ollama for f3"
    assert any("2 figure(s)" in g for g in gaps)
