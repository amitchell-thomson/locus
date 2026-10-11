# Migration checklist (Phase 0, Alec)

Tick these off in order. Nothing here needs kairos running. Steps marked 📱 are on the iPhone, 💻 on the Mac.

## 1. Remove the clutter

- [ ] 💻 Calendar.app → right-click **timetable_MT24_1st_Year** → *Delete*; same for **timetable_HT25_1st_Year**. (If Exchange won't let you delete them, delete them in Outlook on the web, `outlook.office.com` → Calendar → right-click → *Remove*.)
- [ ] 📱 Settings → Apps → Calendar → **Siri** (or *Siri & Search*) → turn off *Learn from this App*, *Show in App*, *Suggest App*, and **Show Siri Suggestions in App**. Settings → Siri → *Suggestions* → turn off *Suggest when sharing* / *Show suggestions* for Calendar.
- [ ] 💻 Calendar → Settings → **General** → untick *Show Birthdays calendar*; **Accounts** → *On My Mac* / Siri Suggestions → uncheck **Show Siri Suggestions calendar**; in the sidebar untick **Scheduled Reminders**.
- [ ] 📱 Calendar → *Calendars* (bottom) → untick **Birthdays**, **Siri Suggestions**, **Scheduled Reminders**.
- [ ] 💻 Calendar sidebar → Google (personal) → right-click **Holidays in Austria** → *Unsubscribe*, or at calendar.google.com → Settings → *Holidays in Austria* → *Remove calendar*.

## 2. Oxford Exchange account

- [ ] 💻 Open the **Extracurriculars** calendar. Note anything dated from today on (keep the list for step 4).
- [ ] After step 4 has the new key calendars, move those future events (drag in Calendar.app, or open each → *Calendar* → pick the key).
- [ ] 💻 System Settings → Internet Accounts → Oxford (Exchange) → turn **Calendars** off. 📱 Settings → Apps → Calendar → Calendar Accounts → Oxford → **Calendars** off. (Mail can stay on.)

## 3. Personal = inbox

- [ ] 💻 Calendar.app → right-click **Privat** → *Get Info* → rename to **Personal**.
- [ ] 💻 Calendar → Settings → General → **Default calendar: Personal**.
- [ ] 📱 Settings → Apps → Calendar → **Default Calendar: Personal**.

## 4. Create the key calendars (iCloud)

- [ ] 💻 File → New Calendar → **iCloud**, one each: `Engineering`, `Careers`, `Sport`, `Travel`, `OQTS`, `Social`, and **`ZZ-kairos-test`** (exact spelling — it's where kairos writes until you flip `live = true`).
- [ ] (Optional) give them the kairos colours so the phone matches the TUI: Personal lavender `#b4befe`, Engineering blue `#89b4fa`, Careers peach `#fab387`, Sport green `#a6e3a1`, Travel sapphire `#74c7ec`, OQTS mauve `#cba6f7`, Social pink `#f5c2e7`, Family yellow `#f9e2af`.
- [ ] 💻 Back to step 2: move the future Extracurriculars events into the right keys.

## 5. Secrets (never in the repo)

### iCloud app-specific password
- [ ] appleid.apple.com → Sign-In and Security → **App-Specific Passwords** → `+` → name it `kairos`. Copy it.
- [ ] 💻 Mac Keychain (for the read-only pull):
  ```sh
  security add-generic-password -U -s kairos-icloud -a "$USER" -w 'xxxx-xxxx-xxxx-xxxx'
  security add-generic-password -U -s kairos-apple-id -a "$USER" -w 'you@icloud.com'   # your Apple ID email
  ```
- [ ] compute-node:
  ```sh
  mkdir -p ~/.config/kairos/secrets && chmod 700 ~/.config/kairos/secrets
  printf '%s' 'xxxx-xxxx-xxxx-xxxx' > ~/.config/kairos/secrets/icloud-app-password
  printf '%s' 'you@icloud.com'      > ~/.config/kairos/secrets/apple-id
  chmod 600 ~/.config/kairos/secrets/*
  ```

### Google secret iCal addresses (read-only imports)
For each calendar: calendar.google.com → ⚙ Settings → (left) the calendar → **Integrate calendar** → **Secret address in iCal format**. (For the OQTS Workspace calendars, sign in as the Workspace account; if the secret address is hidden, a Workspace admin has disabled it — tell Claude and we'll use the public address or another route.)

| kairos source | Google calendar | Keychain service / server file |
|---|---|---|
| `oqts-shared` | `oqts@oqts.org` | `kairos-ics-oqts-shared` / `ics-oqts-shared` |
| `oqts-alec` | `alec@oqts.org` | `kairos-ics-oqts-alec` / `ics-oqts-alec` |
| `gmail` | `alec.mitchellthomson@gmail.com` | `kairos-ics-gmail` / `ics-gmail` |
| `uk-holidays` | Holidays in the UK | public URL, already in config |

```sh
# Mac
security add-generic-password -U -s kairos-ics-oqts-shared -a "$USER" -w 'https://calendar.google.com/calendar/ical/…/basic.ics'
security add-generic-password -U -s kairos-ics-oqts-alec   -a "$USER" -w '…'
security add-generic-password -U -s kairos-ics-gmail       -a "$USER" -w '…'
# compute-node
printf '%s' '…' > ~/.config/kairos/secrets/ics-oqts-shared   # etc., chmod 600
```

### kairos API token
Already generated on compute-node at `~/.config/kairos/secrets/api-token` (see docs/deploy.md). Copy it to the Mac Keychain:
```sh
security add-generic-password -U -s kairos-api-token -a "$USER" -w "$(ssh compute-node cat ~/.config/kairos/secrets/api-token)"
```

## 6. Engineering timetable

- [ ] Get this year's timetable as `.ics` (Engineering Science timetable page / Oxford "My Timetable" export, or the department's calendar subscription link).
- [ ] Put it in `~/local-projects/kairos/fixtures/raw/timetable.ics` and tell Claude: it gets imported into **Engineering** (one-off import through kairosd, so it's logged and undoable) and an anonymised copy becomes a golden fixture.

## 7. Golden fixtures from your real calendars

- [ ] 💻 Calendar.app → select a calendar → File → Export → Export… → save into `~/local-projects/kairos/fixtures/raw/` (git-ignored). Good candidates: Personal, Family, one Google calendar, last term's timetable.
- [ ] Then:
  ```sh
  cd ~/local-projects/kairos
  cargo xtask anonymise fixtures/raw fixtures/golden --salt "$(openssl rand -hex 8)"
  grep -i -E 'alec|oxford|<a name you know>' fixtures/golden/*.ics   # should print nothing
  cargo test -p kairos-core --test semantics every_fixture_round_trips_losslessly
  ```

## 8. Phone (after kairosd is live)

- [ ] Install **ntfy** from the App Store → subscribe to the topic in `~/.config/kairos/secrets/ntfy-topic` on compute-node (server `https://ntfy.sh`).
- [ ] Build the **Add task** Shortcut: `docs/shortcut.md`.
