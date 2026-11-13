# The locus window (locus plan B.4; Slice 1 step 1.2). Sourced from ~/.config/tmux/tmux.conf:
#   source-file ~/local-projects/locus/crates/locus-tui/tmux/locus.tmux
#
# Session `locus` has one window per view: home, calendar (kairos), mail (keryx), and fix, a
# Claude chat for quick fixes to the other three (fix-chat.md). Alt-1..4 switch them, and only
# while this client is ON the locus session; anywhere else the key goes through to the program,
# so a project session loses nothing. Bare digits stay kairos's filters. A project is its own
# tmux session: Enter on its row in Home switches there (the Project window went 13 Nov).
#
# Overridable before sourcing (the test server uses these):
#   @locus-state-dir   locus's state directory (default ~/.local/state/locus)
#   @locus-tui-bin     the locus-tui binary (default <repo>/target/release/locus-tui)
#   @locus-kairos-cmd  @locus-keryx-cmd   the Calendar and Mail commands

# This file's directory, expanded NOW: `current_file` only exists while the file is being read,
# so a %hidden variable would carry the unexpanded format into every binding.
set -gF @locus-tmux-dir "#{d:current_file}"

bind -n M-1 if -F "#{==:#{session_name},locus}" "run-shell -b '#{@locus-tmux-dir}/locus-session.sh select home'" "send-keys M-1"
bind -n M-2 if -F "#{==:#{session_name},locus}" "run-shell -b '#{@locus-tmux-dir}/locus-session.sh select calendar'" "send-keys M-2"
bind -n M-3 if -F "#{==:#{session_name},locus}" "run-shell -b '#{@locus-tmux-dir}/locus-session.sh select mail'" "send-keys M-3"
bind -n M-4 if -F "#{==:#{session_name},locus}" "run-shell -b '#{@locus-tmux-dir}/locus-session.sh select fix'" "send-keys M-4"
# Left from when there were five views: M-5 goes straight to the program again, and the hooks
# that told the Project window which session it followed (slot 42) are gone with it.
unbind -n M-5
set-hook -gu client-session-changed[42]
set-hook -gu client-attached[42]

# Landing on the locus session runs `ensure`: after a restart, tmux-resurrect brings the views
# back as bare shells, and this respawns them (locus-session.sh says how it tells them apart).
set-hook -g client-session-changed[43] "run-shell -b '#{@locus-tmux-dir}/locus-session.sh landed #{q:client_session}'"
set-hook -g client-attached[43] "run-shell -b '#{@locus-tmux-dir}/locus-session.sh landed #{q:client_session}'"
