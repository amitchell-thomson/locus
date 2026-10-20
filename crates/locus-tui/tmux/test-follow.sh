#!/bin/sh
# Step 1.2's check, on a private tmux server so nothing moves on the real screen:
# the locus session's windows, the Project view following a client from quast to nabla,
# and `m` pinning it. Run from anywhere: crates/locus-tui/tmux/test-follow.sh
set -e
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
state=$(mktemp -d)
T="tmux -L locus-test -f /dev/null"
cleanup() { $T kill-server 2>/dev/null || true; kill "$client" 2>/dev/null || true; pkill -f "sleep 600" 2>/dev/null || true; rm -rf "$state"; }
trap cleanup EXIT
$T kill-server 2>/dev/null || true

$T new-session -d -s quast -x 95 -y 38
$T new-session -d -s nabla -x 95 -y 38
$T set -g default-size 95x38
$T set -g @locus-state-dir "$state"
$T set -g @locus-tui-bin "$repo/target/debug/locus-tui"
$T set -g @locus-kairos-cmd "sh -c 'echo kairos stand-in; sleep 600'"
$T set -g @locus-keryx-cmd "sh -c 'echo keryx stand-in; sleep 600'"
$T source-file "$here/locus.tmux"
$T run-shell "$here/locus-session.sh ensure"

echo "== windows in session locus"
$T list-windows -t =locus -F '#{window_index} #{window_name}'

# A real client, on a pseudo-terminal, attached to quast: the "project window". Its stdin is
# held open by sleep: at EOF, script sends ^D through the pty and the quast shell exits.
sleep 600 | TERM=xterm-256color script -q /dev/null $T attach -t quast >/dev/null 2>&1 &
client=$!
sleep 1.5
tty=$($T list-clients -F '#{client_tty} #{session_name}' | awk '$2=="quast"{print $1}')

view() { sleep 1; $T capture-pane -p -t =locus:=project | head -1; }
echo "== project window on quast";   view
$T switch-client -c "$tty" -t nabla
echo "== switched to nabla";         view
echo "   focus file: $(cat "$state/focus")"
$T switch-client -c "$tty" -t locus
echo "== client on the locus session itself (ignored)"; view
$T send-keys -t =locus:=project m
sleep 6   # past the 5 s quiet window that the keypress started
$T switch-client -c "$tty" -t quast
echo "== pinned with m, then switched to quast"; view
$T send-keys -t =locus:=project m
sleep 6
echo "== unpinned, quiet for 5 s: catches up"; view

# Alt-1..4, through the client's own key table (send-keys -K), as if typed.
current() { sleep 0.5; $T list-windows -t =locus -F '#{?window_active,#{window_name},}' | grep .; }
$T switch-client -c "$tty" -t locus
$T send-keys -K -c "$tty" M-3
echo "== on the locus session, Alt-3 shows: $(current)"
$T send-keys -K -c "$tty" M-2
echo "== Alt-2 shows: $(current)"
$T switch-client -c "$tty" -t quast
$T send-keys -K -c "$tty" M-4
echo "== on quast, Alt-4 leaves locus on: $(current) (the key went to quast's pane instead)"
$T kill-window -t =locus:=mail
$T switch-client -c "$tty" -t locus
$T send-keys -K -c "$tty" M-4
echo "== mail window closed, Alt-4 brings it back: $(current)"

# A restart, as tmux-resurrect leaves it: windows with the right names, running bare shells,
# without the @locus-view mark. Landing on the locus session respawns them.
$T switch-client -c "$tty" -t quast
for v in calendar mail; do $T respawn-window -k -t "=locus:=$v" sh; $T set -wu -t "=locus:=$v" @locus-view; done
echo "== after a mock restore, calendar runs: $($T display -p -t =locus:=calendar '#{pane_current_command}')"
$T switch-client -c "$tty" -t locus
sleep 1
echo "== client lands on locus; calendar shows: $($T capture-pane -p -t =locus:=calendar | head -1)"
echo "   mail shows: $($T capture-pane -p -t =locus:=mail | head -1)"

# Home hands a project to the Project window (round 1): Enter on a project row there switches to
# the project window showing it; Esc goes back to Home and the Project view follows again.
$T switch-client -c "$tty" -t locus
$T send-keys -K -c "$tty" M-1
sleep 0.5
$T send-keys -t =locus:=home p Down Enter
echo "== Enter on Home's second project shows: $(current) · $($T capture-pane -p -t =locus:=project | head -1 | grep -o 'from home[^╮]*' | head -1)"
$T send-keys -t =locus:=project Escape
echo "== Esc there shows: $(current) · project header: $($T capture-pane -p -t =locus:=project | head -1 | cut -c1-40)"
