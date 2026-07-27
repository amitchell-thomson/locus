# Agent-layer Phase 0 — verify + size (go/no-go findings)

> Status: **complete, 2026-07-24.** Overall verdict: **GO**, with one gating item (v6 renderer
> fidelity) to resolve at the top of Phase 1. Spikes per the plan §17 Phase 0 / §18.
> Throwaway harness: `scripts/benchmarks/routing_spike.py` (+ `routing_spike_out.json`).
> This is a decision record; the plan is `docs/obsidian-agent-layer-plan.md`.

## Verdict per spike

| Spike | Verdict | One-line |
|---|---|---|
| (a) rmapi pull/push | **GO** | `rmapi` v0.0.34 installed + authed; pull works, full device tree lists. |
| (b) transcription WER | **GO on model, BLOCKER on renderer** | Clean pages transcribe ~perfectly; the v6 **renderer** corrupts edited pages. |
| (c) per-pass routing + cost | **GO → Haiku, not Sonnet** | Haiku beats local *and* Sonnet on the durable passes; bulk ≈ $15–20 on Batch. |
| (d) budget-guard detection | **GO** | The `claude -p` envelope reports cost + token usage + service_tier per call. |

---

## (a) rmapi — GO

- Installed `ddvk/rmapi` v0.0.34 (`~/.local/bin/rmapi`), linux-amd64. Owner did the one-time
  device auth. Non-interactive command surface (`ls`/`stat`/`get`/`geta`/`put`/`find`/`mget`).
- Pull verified against `brevan_howard/Jargon Sheet` (a real handwritten notebook).
- Device folder layout already matches the loops: `rough_notes`, `reading_list`, `careers`,
  `engineering`, `projects`, `quantum_ml`, `brevan_howard`.
- **`geta` (render-annotations-to-PDF) fails on pure notebooks** ("archive does not contain a
  unique pagedata file") — it is for annotated PDFs (Loop B), not handwritten notebooks (Loop A).
  Loop A must use `get` (raw `.rmdoc`) + a stroke renderer → see (b).

## (b) Transcription — GO on quality, RENDERER is the blocker

Pipeline proven end-to-end: `rmapi get` → unzip `.rmdoc` → `rmc` v6→SVG → `cairosvg` SVG→PNG →
vision transcribe.

- **Model quality: GO.** Owner's verdict on the machine transcription of a real page: legible
  content is "essentially perfect." The `[illegible]`→agent-fill approach for genuine gaps is
  endorsed (a later grounded pass fills flagged gaps accurately).
- **BLOCKER — v6 renderer fidelity.** The `[illegible]` regions were **not** the owner's content;
  they are a **rendering artifact**. On the device the page is plain, non-overlapping text; the
  render shows phantom overlap where the owner erased-and-rewrote.
  - Root cause: `rmscene`/`rmc` warns *"some data has not been read — newer format than this
    reader supports."* **Latest `rmscene` (0.8.0) hits the identical wall** as the pinned 0.6.1
    (same warning, same 1189 present / 53 deleted line items). The current reMarkable firmware's
    `.rm` format is not fully parsed by any open-source reader, and the unparsed records appear
    to include erase/scene-cleanup ops → erased strokes get drawn as ink.
  - **Phase-1 direction:** do **not** re-derive the render from raw strokes. Get the reMarkable's
    own render (device/app export path) so the correct proprietary renderer produces the raster;
    that sidesteps the format-parsing problem entirely. (Re-evaluate a firmware-matching OSS
    renderer as a fallback.)
  - The transcription **model is not the bottleneck** — the renderer is. Once a faithful raster
    exists, transcription is a GO.
- **Cost note:** production transcription must attach the page as a **base64 image to one SDK
  vision call** (~$0.01/page on Sonnet). The Phase-0 probe used `claude -p`'s Read tool, which
  spun an 8-turn agentic loop and cost **$0.23/page** — the wrong channel for this.

## (c) Per-pass routing + cost — GO → **Haiku default; Sonnet not justified**

Method: monkeypatch each ingest pass's `generate_structured` with a `claude -p` shim (exact
shipped prompts, schemas, and post-filters — zero drift), run 3 representative live sections
(math / prose / owner-note) through local qwen · Haiku 4.5 · Sonnet 5, grade every output with a
fixed Sonnet grader reusing `locus.eval.judge`'s rubric.

Quality (mean judge score over the 3 sections):

| engine | overall | summ.faith | prop.faith | prop.atom | self-contain | ent.recall | ent.prec |
|---|---|---|---|---|---|---|---|
| local qwen | 3.89 | 4.33 | 4.67 | 4.0 | 4.0 | 4.0 | **2.33** |
| **Haiku 4.5** | **4.61** | **4.67** | **5.0** | 3.67 | **5.0** | **4.67** | 4.67 |
| Sonnet 5 | 4.28 | 4.33 | 4.67 | 4.0 | 4.33 | 3.33 | **5.0** |

- **Haiku scored highest overall — above local AND Sonnet** — and is 3× cheaper. It fixes local's
  §11.B weak spot (entity precision 4.67 vs 2.33).
- Sonnet's only edge is entity precision (5.0); it loses on entity recall (3.33 vs 4.67).
- **Routing decision:** Haiku default across the durable passes (summaries, propositions,
  entities, synthesis, gaps, concepts). The §7 "escalate to Sonnet on eval evidence" clause is
  **not** triggered by this sample — Sonnet is not better on the durable passes here.
- Caveat: n=3 sections, single grader that is itself Sonnet (yet Haiku still won, which
  strengthens the call). **Grow the judge eval before locking routing.**

Cost (sized from real tokens: 3033 sections × 3 durable passes ≈ 9.1k calls, ~1.1k in / ~400 out):

| channel | full bulk re-ingest |
|---|---|
| **Haiku via API Batch** (−50%: $0.50/$2.50 per M) | **~$15–20** |
| Sonnet via Batch ($1.50/$7.50, intro $1/$5) | ~$40–50 |
| via `claude -p` (harness-taxed ~$0.03–0.06/call) | **~$360** (20×) |

The 20× gap is the measured proof of §7's channel split: **bulk → API Batch; ongoing daily
(low volume) → subscription `claude -p`** (cents/day).

## (d) Budget-guard detection — GO

The `claude -p` JSON envelope reports, per call: `total_cost_usd`, full `usage` (input/output/
cache-creation/cache-read tokens), `service_tier`, and `api_error_status`. A local cost/token
ledger (`agent/budget.py`) is a direct read of the envelope — no need to scrape rate-limit
errors as the primary mechanism. Only the **live-throttle error shape** remains to observe (needs
a real throttle event).

---

## Cross-cutting engineering findings (for `agent/claude.py`, §10)

1. **Scrub `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` from the `claude -p` subprocess env.**
   `locus.config.load()` injects the project `.env` key into `os.environ`; the subprocess
   inherits it and `claude -p` prefers the metered key over the subscription OAuth login —
   silently rerouting to metered billing (or failing outright). The runner MUST pass a cleaned
   env. (This bit the spike: every call failed until the env was scrubbed.)
2. **`claude -p` errors are transient** — one call in a 27-call run exited nonzero with an
   incidental connector warning, then succeeded on retry. Bounded retry-then-degrade is
   mandatory (mirrors the ingest §7 contract), and results must be persisted incrementally so a
   late failure never discards earlier work.
3. **Channel choice is per-task:** high-volume small passes and image-transcription both belong
   on the SDK/Batch path (minimal prompt, real tokens), NOT `claude -p` — its ~17–23k-token
   Claude Code harness prompt is cache-created every fresh invocation and dominates small-task
   cost.

## Confirmed pricing (via claude-api skill, 2026-07)

Haiku 4.5 $1/$5 per M · Sonnet 5 $3/$15 (intro $2/$10 through 2026-08-31) · Batch −50% on both.

## Renderer blocker — Phase-1 resolution direction (researched 2026-07-24)

Two prongs investigated; conclusion is to use the **device's own renderer, reached over a tailnet**.

- **No cloud/API server-side render exists.** The reMarkable cloud stores raw strokes only;
  rendering is client-side proprietary. reMarkable's *own* developer docs expose only the raw file
  store (`/home/root/.local/share/remarkable/xochitl`) — **no render/export endpoint**. Every
  "export PDF" tool (rmrl, remt, rmc, remarkable-mcp) renders locally from `.rm`.
- **Re-deriving an OSS renderer is high-effort + brittle.** `rmc` 0.3.0 (Mar 2025) targets firmware
  "software v3"; `rmscene` 0.8.0 still can't fully parse current-firmware files (the "data not read"
  warning; open issue #7 "unknown move_id on line items" since 2023 — the erase/scene records that
  cause our phantom overlap). It's a moving target that shifts each firmware update. Fallback only.
- **Chosen direction: the device's own (faithful) renderer, reached over Tailscale.**
  - Reachability and rendering are *separable*: cloud sync already delivers raw strokes from
    anywhere; the only reason to reach the device is that it alone renders faithfully.
  - **Transport = Tailscale on the tablet** (Toltec/entware; userspace-networking — no TUN in the
    rM kernel; Dropbear old-crypto SSH flags). Solves "device not on home WiFi" — the always-on
    server reaches it by tailnet IP from anywhere the tablet has internet. Firmware OTA wipes the
    install → **disable auto-update**, reinstall after manual updates (check Toltec compat matrix).
  - **Design constraint = sleep.** The rM auto-suspends and drops WiFi/`tailscaled` when asleep, so
    it is only reachable *awake + online*. → prefer **device-pushes-when-awake** (an on-device hook
    renders new/changed notebooks with the device renderer and pushes PDFs to the server over the
    tailnet) over server-pull-anytime.
  - **Still to prototype:** the exact on-device render *trigger* (web-export endpoint rebound off
    `usb0` / driving `xochitl` / `goMarkableStream` framebuffer — framebuffer is current-page only).
    Tailscale makes all reachable; which is cleanest is the Phase-1 experiment.

### Transport SET UP + partially resolved (2026-07-27 session)

Device is a **reMarkable Paper Pro** (`imx8mm-ferrari`, aarch64, Codex Linux 5.7.126 / fw 3.27.3.0,
`/home` 46 GB free). Actual setup done this session:

- **Root SSH** enabled (dev-mode unlock — the owner did the destructive unlock, restored from cloud;
  fine). USB SSH at `10.11.99.1`; WLAN SSH enabled via `rm-ssh-over-wlan on`.
- **WiFi client-isolation is ON** on the home network — server and tablet on the same `192.168.1.x`
  subnet **cannot** reach each other (ARP FAILED both directions). → the LAN path is dead;
  **Tailscale is required**, not optional.
- **Toltec is dead** for this firmware (max fw 3.3.2.1666; we're on 3.27). Successor **Vellum**
  exists, but we skipped a package manager entirely and installed the **static Tailscale binary**
  (`tailscale_1.98.9_arm64`) into `/home/root` — self-contained, uninstall = delete the folder, no
  system modification. Runs via `nohup tailscaled` (userspace) — **not yet reboot-persistent**
  (Phase-1 hardening: init hook + firmware-update survival).
- **HARD constraint: this kernel has NO TUN** (`CONFIG_TUN` not built, `modprobe tun` fails). Forces
  Tailscale **userspace-networking** mode → the tablet has no transparent tailnet routing either
  direction. **Inbound to the tablet does not work**: Tailscale SSH hangs at banner; `tailscale serve
  --tcp 22` forwards 0 bytes (tried localhost / 127.0.0.1 / [::1] backends). Sourcing a matching
  `tun.ko` for `6.12.49-imx8mm-ferrari` is possible but uncertain and **would not change the
  architecture** (tablet still sleeps) — not pursued.
- **OUTBOUND device-push PROVEN.** `tailscale nc 100.117.10.28 22` from the tablet returned the
  server's `SSH-2.0-OpenSSH_9.6p1` banner → the tablet can push to the server over the tailnet in
  userspace mode. This is the correct direction anyway (sleep → device-pushes-when-awake). Server
  runs sshd:22, tailnet IP `100.117.10.28`; tablet `remarkable` = `100.104.8.98`.
- Server-side ready: dedicated RSA key `~/.ssh/id_rsa_remarkable` + `Host remarkable` alias (built
  for the abandoned inbound path; the outbound path will instead need a **tablet→server** key the
  server trusts — Phase 1). NOTE: `agent/claude.py`-style env scrubbing does not apply here.

**Phase-1 transport TODO:** (1) make tailscaled reboot- and firmware-update-persistent; (2) tablet→
server auth (key the server trusts) for the push; (3) a socks-capable pusher on the tablet — it has
only `wget` (no curl/rsync/ssh-client confirmed): use tailscaled `--socks5-server` + a socks tool, or
`tailscale nc` as an ssh `ProxyCommand`. (4) then the render trigger.

### Render trigger — investigated 2026-07-27, NOT yet cracked (Phase-1 continues)

The faithful-render path is the reMarkable **USB Web Interface** — xochitl's own HTTP API that serves a
**device-rendered PDF** (faithful, matches the screen). Confirmed endpoints (docs): `GET /documents/`
(list), **`GET /download/{uuid}/pdf`** (device-rendered PDF — the one we want), `GET
/download/{uuid}/rmdoc` (raw archive), `POST /upload` — on **port 80 at the usb0 IP** (10.11.99.1).
Gated by **`WebInterfaceEnabled=true`** in `/home/root/.config/remarkable/xochitl.conf` AND an IP on
`usb0`; xochitl starts it at boot/when usb0 comes up.

Tonight: set `WebInterfaceEnabled=true` (conf backed up at `xochitl.conf.locus.bak`), `systemctl
restart xochitl` — but **:80 did NOT bind** (connection refused on 10.11.99.1 and 127.0.0.1). Likely
xochitl binds the web interface only on a usb0 *up-event*, not when usb0 already has an IP; or the
Paper Pro differs. Separately, there IS a live xochitl HTTP service on **`127.0.0.1:8787`** but it is
**not** the web-interface API (`/documents/` and `/download/{uuid}/pdf` both 404 there) — unknown
purpose; may have a render endpoint under a different path.

**WHY :80 didn't bind (research 2026-07-27 — resolved):** xochitl starts the web interface only when
(a) `WebInterfaceEnabled=true` AND (b) `usb0` has an IP, and it makes that decision **at xochitl
startup / on a usb0 up-event**. Our restart didn't trigger it because usb0 already had its IP (no
up-event at the checkpoint). Confirmed community behavior: "you need to plug in the USB cable once to
toggle the web interface on." The interface **binds to the usb0 IP (10.11.99.1)**.

**The clean device-push render design (no cable, no WiFi exposure):** the `rM-self-serve/webinterface-
onboot` *technique* is just — assign `10.11.99.1/32` to `usb0` at boot, then xochitl starts the web
interface on it **without a cable** (usb0 the gadget iface exists unplugged; it just lacks an IP). Then
`http://10.11.99.1` is a **device-local** address the tablet can fetch from itself. So:
1. boot hook: `ip addr add 10.11.99.1/32 dev usb0` + `WebInterfaceEnabled=true` + one xochitl start →
   web interface stays up on 10.11.99.1 (no periodic xochitl restart needed).
2. render pass (device-side): `wget http://10.11.99.1/download/{uuid}/pdf` → **faithful device-rendered
   PDF** (the device's own renderer — no phantom overlap by construction).
3. push to server over the tailnet (`tailscale nc`, already proven). Only the tailnet push leaves the
   device; 10.11.99.1 never touches WiFi → no `webinterface-wifi` needed, no unauth exposure.

**Render-trigger Phase-1 steps (concrete, in order):**
1. **Prove faithfulness fast:** with the flag set, **re-plug the USB cable** → `:80` should bind →
   `wget http://10.11.99.1/download/<uuid>/pdf` on the device → push to server → confirm NO phantom
   overlap vs the OSS `rmc` render. This is the acceptance test that closes the blocker.
2. **Make it cable-free:** replicate the onboot technique (assign `10.11.99.1/32` to usb0 at boot +
   start xochitl once). Verify usb0 can hold the IP while unplugged on the Paper Pro (empirical — the
   packaged tool is confirmed only for rM1/rM2 ≤/≥v2.15, NOT yet Paper Pro / fw 3.x, so test the raw
   technique).
3. Wire the periodic render+push (device-side, over tailnet) into the capture loop.

Notes: `127.0.0.1:8787` is a live xochitl HTTP service but NOT the web-interface API (endpoints 404);
purpose unidentified — ignore. `webinterface-wifi` exists (exposes :80 over WiFi with auth) but is
**not needed** given the localhost-fetch + tailnet-push design. Fallback if usb0-IP-while-unplugged
fails on Paper Pro: `goMarkableStream` framebuffer (faithful, current-page only), or render only while
docked.

### SOLVED 2026-07-27: faithful device render WORKS on the Paper Pro via the USB web interface

**The blocker is CRACKED.** Root cause of the earlier failures: our `echo 'WebInterfaceEnabled=true'
>> xochitl.conf` appended the key to the END of the file, which put it under the `[Tooltips]` INI
section. xochitl uses Qt QSettings (section = last `[Header]` above the key), so it read it as
`[Tooltips].WebInterfaceEnabled` and **never saw the setting** — every prior test had the key
misplaced. Moving it into **`[General]`** (with usb0 up) + `systemctl restart xochitl` → **`:80` bound
on `10.11.99.1`** and `wget http://10.11.99.1/download/{uuid}/pdf` returned a **422,988-byte, valid
`%PDF-1.7`, 5-page** device-rendered PDF. Pushed it to the server over the tailnet and rendered page 1:
**completely clean — zero phantom overlap**, every entry legible in true on-device order (vs the OSS
`rmc` render's garbled erased-stroke band). Confirmed via `xochitl` binary strings: it embeds
`/pdfrenderer.cpp`, `/export/`, `/download/` — the PDF comes from **xochitl's own renderer**, faithful
by construction. Binary confirms the only config key is `WebInterfaceEnabled` (no others).

**Working recipe (device-side):**
1. `xochitl.conf` → `[General]` section has `WebInterfaceEnabled=true` (NOT under any other section).
2. usb0 has an IP (10.11.99.1) — present while USB-connected; see Phase-1 for cable-free.
3. `systemctl restart xochitl` (once) → web interface binds `10.11.99.1:80`.
4. `wget http://10.11.99.1/download/<uuid>/pdf` → faithful device-rendered PDF (also `/rmdoc` raw,
   `/documents/` JSON list, `/thumbnail/`, `POST /upload`).
5. Push to server over the tailnet (`tailscale nc` — proven). Only the tailnet push leaves the device.

**Remaining for production (Phase 1 — the render itself is DONE):**
1. **Cable-free `:80` bind** — the web interface currently needs usb0 to have an IP. Assign
   `10.11.99.1/32` to usb0 at boot (the `webinterface-onboot` technique — usb0 the gadget iface exists
   unplugged; it just needs the IP), so it binds without a cable when the tablet's on WiFi. Verify the
   Paper Pro holds the usb0 IP while unplugged (empirical).
2. Avoid the periodic `systemctl restart xochitl` (disruptive) — bind once at boot, then just fetch.
3. Get the doc UUIDs to render: `GET http://10.11.99.1/documents/` (JSON) lists them; or drive from
   `rmapi`/the capture loop.
4. Wire fetch-local → tailnet-push into the capture loop (device-push when awake).

Note: `127.0.0.1:8787` was a red herring (different xochitl HTTP service, 404s on these routes) — the
real interface is `10.11.99.1:80`. Config backups: `xochitl.conf.locus.bak`, `.locus.bak2`.
  - Setup safety: the install is additive (packages under entware/opkg), does not modify firmware or
    touch xochitl/notebooks, and is reversible (`opkg remove` / factory reset). SSH is an
    reMarkable-documented feature. Only behavior changes: turn OFF auto-update (manual only), and
    slightly higher battery use if WiFi stays up longer.

## Open before Phase 1

- **Resolve the v6 renderer** (the one gating item) — Tailscale transport + on-device render per
  the section above; prototype the render trigger.
- Grow the judge eval beyond n=3 before locking Haiku routing corpus-wide.
- Observe a live subscription throttle to capture the error shape for the budget guard.
