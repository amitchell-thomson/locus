#!/bin/sh
# Called by tmux when a client lands on a session (locus.tmux). Records it as the project
# window's session unless it is one of the ignored ones (the locus window itself, aulos).
# The write is atomic (tmp + mv) so locus-tui never reads half a name.
session=$1
[ -n "$session" ] || exit 0
ignore=$(tmux show -gqv @locus-ignore)
for s in ${ignore:-locus aulos}; do
    [ "$session" = "$s" ] && exit 0
done
dir=$(tmux show -gqv @locus-state-dir)
dir=${dir:-$HOME/.local/state/locus}
case $dir in "~"*) dir=$HOME${dir#\~} ;; esac
mkdir -p "$dir" && printf '%s\n' "$session" > "$dir/focus.tmp" && mv "$dir/focus.tmp" "$dir/focus"
