# Locus laptop outbox (macOS → server)

Automatically pushes anything you drop into `~/LocusDrop/` on your Mac to the Locus server's
`vault/incoming/` over SSH, where the watcher ingests it. **One-way**: the server can never
delete or modify files on your Mac. Works over any network (it reuses your SSH connection),
and retries automatically when you reconnect — drops made while offline simply wait.

## How it works

- `locus-outbox.sh` runs `rsync --remove-source-files` from `~/LocusDrop/` to the server.
  A file is removed from the Mac **only after** it transfers successfully; if you're offline,
  it stays and the next run retries it.
- A **launchd** agent (`com.locus.outbox.plist`) runs that script at login and every 60s.
- Config (host, paths) lives in `~/.config/locus-outbox/outbox.conf` — not in this repo.

All commands below run **on the Mac**.

## 1. Copy the scripts from the server

```bash
scp -r compute-node:/home/alec/server-projects/locus/scripts/laptop-outbox ~/locus-outbox-setup
cd ~/locus-outbox-setup
```

## 2. Install the script and config

```bash
mkdir -p ~/.local/bin ~/.config/locus-outbox
cp locus-outbox.sh ~/.local/bin/
chmod +x ~/.local/bin/locus-outbox.sh

cp outbox.conf.example ~/.config/locus-outbox/outbox.conf
# Edit REMOTE / REMOTE_DIR / DROP_DIR if your values differ from the defaults:
#   ${EDITOR:-nano} ~/.config/locus-outbox/outbox.conf
```

## 3. Create the drop folder — with the category taxonomy

First-level folders inside the drop folder ARE the document categories: rsync carries the
folder through to `vault/incoming/<folder>/`, and ingest derives `documents.category` from
it (known kind names singularize, `papers` → `paper`; anything else is taken verbatim).
First-level folders persist across flushes — only deeper emptied subfolders are tidied.
Files dropped loose at the root ingest as `uncategorized`.

```bash
mkdir -p ~/LocusDrop/{papers,notes,projects,achievements,cv}
```

Sorting a document into your knowledge base is now one drag: pick the folder, the agent
ships it within a minute, category included.

## 4. Install and start the launchd agent

```bash
# Substitute your home dir into the template, then install it.
sed "s|__HOME__|$HOME|g" com.locus.outbox.plist > ~/Library/LaunchAgents/com.locus.outbox.plist

# Load + enable it (modern launchctl):
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.locus.outbox.plist
launchctl enable gui/$(id -u)/com.locus.outbox
# (On older macOS: launchctl load -w ~/Library/LaunchAgents/com.locus.outbox.plist)
```

## 5. Test it

```bash
echo "hello locus $(date)" > ~/LocusDrop/test.txt
# wait up to ~60s, then check the log and the server:
tail -n 20 ~/Library/Logs/locus-outbox.log
ssh compute-node 'ls -l /home/alec/server-projects/locus/vault/incoming/'
```

You should see `test.txt` arrive on the server and disappear from `~/LocusDrop/`.

## Managing it

```bash
# Force a run now:
launchctl kickstart gui/$(id -u)/com.locus.outbox

# Stop / remove:
launchctl bootout gui/$(id -u)/com.locus.outbox

# Change the interval: edit StartInterval in the installed plist, then bootout + bootstrap again.
```

## Optional — instant push with fswatch

The 60s interval is fine for Locus, but if you want files to leave the moment you drop them:

```bash
brew install fswatch
# Run a watcher that fires the script on any change in the drop folder:
fswatch -o ~/LocusDrop | while read -r _; do ~/.local/bin/locus-outbox.sh; done
```

Wrap that in its own launchd agent if you want it persistent. The interval agent can stay
running alongside as a safety net (it just exits quietly when there's nothing to send).
