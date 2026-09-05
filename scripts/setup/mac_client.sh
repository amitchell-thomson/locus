#!/usr/bin/env bash
# Wire the MacBook's Claude clients to the Locus MCP server on compute-node.
#
# WHY STDIO-OVER-SSH (and not an HTTP server on the box)
# ------------------------------------------------------
# The MCP server has to run WHERE THE CORPUS IS: it needs the SQLite DB, sqlite-vec, Ollama for
# query embedding, the CPU reranker, and rmapi's auth token for the tablet push. None of that can
# move to the laptop. Over SSH the client spawns the server through the Tailscale link that
# already exists and already authenticates — no listening port, no second auth system, no secret
# copied onto the laptop. An HTTP transport would need all three.
#
# Run this ON THE MAC:
#   ssh compute-node 'cat ~/server-projects/locus/scripts/setup/mac_client.sh' | bash
#
# Idempotent: re-running replaces the entries it wrote.
set -euo pipefail

HOST="${LOCUS_HOST:-compute-node}"                                   # Tailscale MagicDNS name
BIN="${LOCUS_BIN:-/home/alec/server-projects/locus/.venv/bin/locus}"  # absolute: ssh gets no login PATH
DESKTOP_CFG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"

# ServerAliveInterval keeps a long conversation's idle server from being reaped mid-session.
SSH_ARGS=(-T -o ConnectTimeout=10 -o ServerAliveInterval=30 "$HOST" "$BIN" mcp)

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

say "1/4  Checking the SSH link to $HOST"
if ! ssh -o BatchMode=yes -o ConnectTimeout=10 "$HOST" true 2>/dev/null; then
  echo "  FAILED: 'ssh $HOST' does not work without a prompt."
  echo "  Fix that first (Tailscale up on both ends; key loaded — 'ssh-add --apple-use-keychain ~/.ssh/id_ed25519')."
  echo "  Claude Desktop cannot answer a password prompt, so this must be non-interactive."
  exit 1
fi
echo "  ok"

say "2/4  Checking the server answers and advertises both tools"
if printf '%s\n' \
     '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"setup","version":"1"}}}' \
     '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
     '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
   | ssh "${SSH_ARGS[@]}" 2>/dev/null | grep -q '"to_remarkable"'; then
  echo "  ok — capture + to_remarkable are live"
else
  echo "  FAILED: the server did not advertise to_remarkable."
  echo "  On $HOST: git -C ~/server-projects/locus pull, then re-run this."
  exit 1
fi

say "3/4  Registering with Claude Code (user scope — every directory on this Mac)"
if command -v claude >/dev/null 2>&1; then
  claude mcp remove -s user locus >/dev/null 2>&1 || true
  claude mcp add -s user locus -- ssh "${SSH_ARGS[@]}"
  echo "  registered"
else
  echo "  SKIPPED: no 'claude' on PATH. Run this by hand once it is installed:"
  echo "    claude mcp add -s user locus -- ssh ${SSH_ARGS[*]}"
fi

say "4/4  Registering with the Claude desktop app"
mkdir -p "$(dirname "$DESKTOP_CFG")"
[ -f "$DESKTOP_CFG" ] || echo '{}' > "$DESKTOP_CFG"
cp "$DESKTOP_CFG" "$DESKTOP_CFG.bak.$(date +%Y%m%d%H%M%S)"
if command -v python3 >/dev/null 2>&1; then
  CFG="$DESKTOP_CFG" HOST="$HOST" BIN="$BIN" python3 - <<'PY'
import json, os
path = os.environ["CFG"]
with open(path) as fh:
    cfg = json.load(fh) or {}
cfg.setdefault("mcpServers", {})["locus"] = {
    # /usr/bin/ssh, not "ssh": the desktop app spawns servers with a minimal PATH.
    "command": "/usr/bin/ssh",
    "args": ["-T", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=30",
             os.environ["HOST"], os.environ["BIN"], "mcp"],
}
with open(path, "w") as fh:
    json.dump(cfg, fh, indent=2)
    fh.write("\n")
print("  written to", path)
PY
  echo "  RESTART the Claude desktop app — it reads this file only at launch."
else
  echo "  SKIPPED: no python3. Put this in $DESKTOP_CFG under \"mcpServers\":"
  cat <<JSON
    "locus": {
      "command": "/usr/bin/ssh",
      "args": ["-T", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=30",
               "$HOST", "$BIN", "mcp"]
    }
JSON
fi

say "Installing the slash commands for Claude Code on this Mac"
mkdir -p "$HOME/.claude/commands"
for name in locus-capture remarkable; do
  ssh "$HOST" "cat ~/server-projects/locus/.claude/commands/$name.md" > "$HOME/.claude/commands/$name.md"
  echo "  ~/.claude/commands/$name.md"
done

say "Done."
cat <<'EOF'
  In any Claude Code session on this Mac:  /locus-capture   ·   /remarkable [file]
  In the Claude desktop app (after a restart), just ask:
    "save this conversation to Locus"   ·   "send me that on my reMarkable"
EOF
