#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Print one rendered frame of the real mirror:
//! cargo run -p keryx-tui --example frame -- <config> <rfc3339-now> <w> <h> [keys]
//! Keys: letters as typed, `\n` = Enter, `\t` = Tab, `^` = Esc.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let cfg = keryx_core::config::Config::load(std::path::Path::new(&a[1])).unwrap();
    let now = chrono::DateTime::parse_from_rfc3339(&a[2])
        .unwrap()
        .with_timezone(&cfg.tz());
    let db = keryx_store::db::Db::open_readonly(&cfg.db_path()).unwrap();
    let mut app = keryx_tui::app::App::new(cfg, db, now);
    app.dry_open = true;
    for k in a.get(5).map(String::as_str).unwrap_or("").chars() {
        use keryx_tui::app::Key;
        app.key(match k {
            '\n' => Key::Enter,
            '\t' => Key::Tab,
            '^' => Key::Esc,
            c => Key::Char(c),
        });
    }
    print!(
        "{}",
        keryx_tui::render_to_string(&mut app, a[3].parse().unwrap(), a[4].parse().unwrap())
    );
}
