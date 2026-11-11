//! Render the keryx window from the real mirror, as text: `cargo run -p keryx-tui --example dump
//! -- 160 40`. For looking at a layout change on real mail without opening the window.
fn main() -> Result<(), String> {
    let args: Vec<u16> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let (w, h) = (args.first().copied().unwrap_or(120), args.get(1).copied().unwrap_or(40));
    let cfg =
        keryx_core::config::Config::load(&keryx_core::config::Config::default_path()).map_err(|e| e.to_string())?;
    let db = keryx_store::db::Db::open_readonly(&cfg.db_path()).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().with_timezone(&cfg.tz());
    let mut app = keryx_tui::app::App::new(cfg, db, now);
    print!("{}", keryx_tui::render_to_string(&mut app, w, h));
    Ok(())
}
