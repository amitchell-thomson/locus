//! Following the theme, end to end, in its own process (follow_theme is process-wide): an app
//! that follows sees the file's colours, and a new file within half a second, with no restart.

use locus_theme::{c, follow_theme_at};
use ratatui::style::Color;
use std::time::Duration;

#[test]
fn a_following_app_takes_the_files_colours_and_picks_up_a_new_file_live() {
    let dir = std::env::temp_dir().join(format!("locus-theme-follow-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("palette.toml");
    std::fs::write(&file, "name = \"A\"\nbase = \"#282828\"\nblue = \"#83a598\"\n").unwrap();
    follow_theme_at(&file);
    assert_eq!(c("blue"), Color::Rgb(0x83, 0xa5, 0x98));
    assert_eq!(
        c("mauve"),
        Color::Rgb(0xcb, 0xa6, 0xf7),
        "a colour the file leaves out stays Mocha's"
    );

    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&file, "name = \"B\"\nblue = \"#7aa2f7\"\n").unwrap();
    std::thread::sleep(Duration::from_millis(450));
    assert_eq!(
        c("blue"),
        Color::Rgb(0x7a, 0xa2, 0xf7),
        "the new file is read without a restart"
    );

    std::fs::remove_file(&file).unwrap();
    std::thread::sleep(Duration::from_millis(450));
    assert_eq!(c("blue"), Color::Rgb(0x89, 0xb4, 0xfa), "no file: Mocha again");
    let _ = std::fs::remove_dir_all(&dir);
}
