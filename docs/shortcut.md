# iOS Shortcut: "Add task"

Posts a task to kairosd over Tailscale. The scheduler places work blocks; they appear in iOS Calendar after the next sync (~1 min).

**Before you start:** Tailscale on, and you know the API token (`ssh compute-node cat ~/.config/kairos/secrets/api-token`).

Shortcuts app → `+` → name it **Add task**, then add these actions:

1. **Ask for Input** — Prompt: `Task`, Input type: *Text* → rename variable **Title**.
2. **Choose from Menu** — Prompt: `Key`; items: `Personal`, `Engineering`, `Careers`, `Sport`, `Travel`, `OQTS`, `Social`. In each branch add **Text** with the same word. After the menu, rename *Menu Result* → **Key**.
3. **Ask for Input** — Prompt: `Estimate (minutes)`, Input type: *Number*, default `60` → **Estimate**.
4. **Choose from Menu** — Prompt: `Deadline?`, items `Yes`, `No`.
   - *Yes*: **Ask for Input** — Prompt `Due`, Input type *Date and Time* → then **Format Date** → *Custom* format `yyyy-MM-dd'T'HH:mm` → **Text** with the formatted date.
   - *No*: **Text** (empty).
   - After the menu: rename *Menu Result* → **Due**.
5. **Dictionary**: `title` = Title, `key` = Key, `estimate_min` = Estimate (Number), `due` = Due (Text; leave empty for none).
   *(If `due` is empty kairosd ignores it.)*
6. **Get Contents of URL**
   - URL: `http://compute-node:7420/api/tasks` (or `http://100.117.10.28:7420/api/tasks`)
   - Method: **POST**; Request Body: **JSON** → *Dictionary* from step 5
   - Headers: `Authorization` = `Bearer <your token>`, `X-Kairos-Author` = `alec`
7. **Get Dictionary Value** `plan` → **Get Dictionary Value** `summary` → **Show Notification** with it (shows blocks placed and any "N h short before …" warning).

Optional: add the Shortcut to the Home Screen / Action Button, and say "Hey Siri, add task".

**Errors:** a 400 response means kairosd rejected the input (e.g. unknown key); the body has `{"error": "…"}` — add a *Show Result* action while testing.
