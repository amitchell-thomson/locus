# chezmoi templates for kairos

Copy (or symlink) these into `~/.local/share/chezmoi/` and add to `.chezmoiignore`:

```
{{ if ne .chezmoi.os "darwin" }}
Library/LaunchAgents/com.kairos.sync.plist
{{ end }}
{{ if ne .chezmoi.os "linux" }}
.config/systemd/user/kairosd.service
{{ end }}
```

`deploy/kairos/install-chezmoi.sh` does both. See `kairos/docs/deploy.md`.
