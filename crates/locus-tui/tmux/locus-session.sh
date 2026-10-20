#!/bin/sh
# The locus tmux session: one window per view, created when missing (locus.tmux).
#   locus-session.sh ensure          create the session and any missing window
#   locus-session.sh select <view>   ensure, then show that view (home|project|calendar|mail)
# Quitting a view closes its window; its Alt key brings it back.
set -e
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)

opt() { tmux show -gqv "$1"; }
state=$(opt @locus-state-dir); state=${state:-$HOME/.local/state/locus}
case $state in "~"*) state=$HOME${state#\~} ;; esac
bin=$(opt @locus-tui-bin); bin=${bin:-$repo/target/release/locus-tui}
kairos=$(opt @locus-kairos-cmd); kairos=${kairos:-kairos tui}
keryx=$(opt @locus-keryx-cmd)
if [ -z "$keryx" ]; then
    if command -v keryx >/dev/null 2>&1; then keryx=keryx; else keryx=$HOME/local-projects/keryx/target/release/keryx; fi
fi

command_for() {
    case $1 in
        home) echo "LOCUS_STATE_DIR='$state' '$bin' home" ;;
        project) echo "LOCUS_STATE_DIR='$state' '$bin' project" ;;
        calendar) echo "$kairos" ;;
        mail) echo "$keryx" ;;
        *) echo "locus-session.sh: no view '$1' (home|project|calendar|mail)" >&2; exit 2 ;;
    esac
}

# A view that fails keeps its window open on the error (keryx with no config exits at once,
# and a window that vanishes says nothing); a clean quit still closes it.
window_for() {
    echo "$(command_for "$1") || { echo; echo \"$1 exited with an error · Enter closes this window\"; read _; }"
}

# Every window this script starts carries @locus-view. tmux-resurrect restores windows by name
# but not user options, and restores a pane it has no rule for as a bare shell: so a view window
# WITHOUT the mark is one resurrect brought back, and is respawned with its real command.
ensure() {
    if ! tmux has-session -t =locus 2>/dev/null; then
        tmux new-session -d -s locus -n home "$(window_for home)"
        tmux set -w -t "=locus:=home" @locus-view home
    fi
    base=$(tmux show -gv base-index)
    i=0
    for view in home project calendar mail; do
        mark=$(tmux list-windows -t =locus -F '#{window_name} #{@locus-view}' | awk -v v="$view" '$1==v{print $2; exit}')
        if ! tmux list-windows -t =locus -F '#{window_name}' | grep -qx "$view"; then
            tmux new-window -d -t "=locus:$((base + i))" -n "$view" "$(window_for "$view")" 2>/dev/null \
                || tmux new-window -d -a -t "=locus:" -n "$view" "$(window_for "$view")"
            tmux set -w -t "=locus:=$view" @locus-view "$view"
        elif [ -z "$mark" ]; then
            tmux respawn-window -k -t "=locus:=$view" "$(window_for "$view")"
            tmux set -w -t "=locus:=$view" @locus-view "$view"
        fi
        i=$((i + 1))
    done
}

# The attach hook and an Alt key fire together when a client lands on locus and presses one at
# once; two unlocked ensures each create the missing window, and then neither can name it.
locked_ensure() {
    tmux wait-for -L locus-ensure
    ensure || { tmux wait-for -U locus-ensure; exit 1; }
    tmux wait-for -U locus-ensure
}

case ${1:-ensure} in
    ensure) locked_ensure ;;
    # From the attach hooks (locus.tmux): a client landing on locus revives what a restart left.
    landed) [ "${2:-}" = locus ] && locked_ensure; true ;;
    select) command_for "${2:-}" >/dev/null; locked_ensure; tmux select-window -t "=locus:=$2" ;;
    *) echo "usage: locus-session.sh ensure | select <home|project|calendar|mail>" >&2; exit 2 ;;
esac
