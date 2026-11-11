//! Render the keryx window from the real mirror, as text: `cargo run -p keryx-tui --example dump
//! -- 160 40`. For looking at a layout change on real mail without opening the window.
//! A third argument, a thread ref (`personal:12345`), opens that thread; a fourth, a unix time,
//! stands in for when you last read it; a fifth, keys to press first (`j` `k` `z`, `e` for ⏎).
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: u16| args.get(i).and_then(|a| a.parse().ok()).unwrap_or(d);
    let (w, h) = (num(0, 120), num(1, 40));
    let cfg =
        keryx_core::config::Config::load(&keryx_core::config::Config::default_path()).map_err(|e| e.to_string())?;
    let db = keryx_store::db::Db::open_readonly(&cfg.db_path()).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().with_timezone(&cfg.tz());
    let mut app = keryx_tui::app::App::new(cfg, db, now);
    if let Some((acc, tid)) = args.get(2).and_then(|r| keryx_core::model::parse_thread_ref(r)) {
        let mut open = app.load_thread(acc, tid).ok_or("no such thread")?;
        open.last_read = args.get(3).and_then(|t| t.parse().ok());
        app.open = Some(open);
        app.mode = keryx_tui::app::Mode::Thread;
        // Draw once so the selection knows where each email starts.
        let _ = keryx_tui::render_to_string(&mut app, w, h);
        for ch in args.get(4).map(String::as_str).unwrap_or("").chars() {
            let k = if ch == 'e' { keryx_tui::app::Key::Enter } else { keryx_tui::app::Key::Char(ch) };
            app.key(k);
            let _ = keryx_tui::render_to_string(&mut app, w, h);
        }
    }
    print!("{}", keryx_tui::render_to_string(&mut app, w, h));
    Ok(())
}
