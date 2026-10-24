#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Print one rendered frame: cargo run -p kairos-tui --example frame -- <config> <now> <w> <h> [view]
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let cfg = kairos_core::config::Config::load(std::path::Path::new(&a[1])).unwrap();
    let now = chrono::DateTime::parse_from_rfc3339(&a[2])
        .unwrap()
        .with_timezone(&chrono::Utc);
    let mut app = kairos_tui::load(cfg, std::path::Path::new(&a[1]), now).unwrap();
    for k in a.get(5).map(String::as_str).unwrap_or("").chars() {
        app.key(match k {
            '\t' => kairos_tui::app::Key::Tab,
            '\n' => kairos_tui::app::Key::Enter,
            c => kairos_tui::app::Key::Char(c),
        });
    }
    print!(
        "{}",
        kairos_tui::render_to_string(&app, a[3].parse().unwrap(), a[4].parse().unwrap())
    );
}
