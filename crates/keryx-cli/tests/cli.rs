#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The `keryx` binary against the fixture mirror.

use serde_json::Value;
use std::process::Command;

struct Env {
    _dir: tempfile::TempDir,
    config: std::path::PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let (cfg, _) = keryx_store::testkit::fixture_db_file(&dir.path().join("data"));
    let example = std::fs::read_to_string(keryx_store::testkit::root().join("config/config.example.toml")).unwrap();
    let toml = example.replace(
        "[server]",
        &format!(
            "[paths]\ndata_dir = {:?}\n\n[server]",
            cfg.paths.data_dir.display().to_string()
        ),
    );
    let config = dir.path().join("config.toml");
    std::fs::write(&config, toml).unwrap();
    Env { _dir: dir, config }
}

fn keryx(e: &Env, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_keryx"))
        .arg("--config")
        .arg(&e.config)
        .args(args)
        .env("KERYX_NOW", "2026-10-14T15:00:00+01:00")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn inbox_json_schema() {
    let e = env();
    let (ok, out, err) = keryx(&e, &["inbox", "--json", "--limit", "5"]);
    assert!(ok, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["schema"], 1);
    let t = v["threads"].as_array().unwrap();
    assert_eq!(t.len(), 5);
    for row in t {
        for k in [
            "n",
            "ref",
            "account",
            "thread_id",
            "subject",
            "who",
            "date",
            "count",
            "unread",
            "flagged",
            "keys",
        ] {
            assert!(row.get(k).is_some(), "missing {k}: {row}");
        }
        // RFC 3339 with the London offset.
        let d = row["date"].as_str().unwrap();
        assert!(d.ends_with("+01:00") || d.ends_with("+00:00"), "{d}");
    }
}

#[test]
fn at_n_refers_to_the_last_listing() {
    let e = env();
    let (ok, out, _) = keryx(&e, &["search", "subject:\"group project\""]);
    assert!(ok);
    assert!(out.starts_with("@1 "), "{out}");
    let (ok, shown, err) = keryx(&e, &["show", "@1"]);
    assert!(ok, "{err}");
    assert!(shown.contains("Subject: Group project plan"));
    assert!(shown.contains("Subject: Re: Group project plan"));
    let (ok, js, _) = keryx(&e, &["show", "@1", "--json"]);
    assert!(ok);
    let v: Value = serde_json::from_str(&js).unwrap();
    assert_eq!(v["messages"].as_array().unwrap().len(), 2);
    assert_eq!(v["messages"][0]["reply_to"][0]["email"], "jane@personal.example.com");
    let (ok, _, err) = keryx(&e, &["show", "@99"]);
    assert!(!ok && err.contains("the last listing had 1 rows"), "{err}");
}

#[test]
fn errors_are_explained() {
    let e = env();
    let (ok, _, err) = keryx(&e, &["search", "form:jane"]);
    assert!(!ok && err.contains("unknown search operator"), "{err}");
    let (ok, _, err) = keryx(&e, &["inbox", "--account", "hotmail"]);
    assert!(
        !ok && err.contains("accounts: personal, oxford, oqts, oqts-shared"),
        "{err}"
    );
    let (ok, _, err) = keryx(&e, &["show", "nowhere:1"]);
    assert!(!ok && err.contains("no account"), "{err}");
}

#[test]
fn addresses_for_completion() {
    let e = env();
    let (ok, out, _) = keryx(&e, &["addresses", "ja", "--json"]);
    assert!(ok);
    let v: Value = serde_json::from_str(&out).unwrap();
    let c = &v["contacts"][0];
    assert_eq!(c["email"], "jane.doe@example.com");
    assert_eq!(c["display"], "Jane Doe <jane.doe@example.com>");
    let (_, plain, _) = keryx(&e, &["addresses", "sam"]);
    assert_eq!(plain.trim(), "Sam <sam@example.com>");
}

#[test]
fn account_by_name_and_text_output() {
    let e = env();
    let (ok, out, _) = keryx(&e, &["inbox", "--account", "OQTS"]);
    assert!(ok);
    assert!(out.contains("Poster for the talk"), "{out}");
    assert!(!out.contains("Oxford "), "{out}");
}

#[test]
fn kairos_commands_explain_themselves() {
    let e = env();
    let (ok, out, err) = keryx(&e, &["suggestions", "--json"]);
    assert!(ok, "{err}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["schema"], 1);
    assert!(v["suggestions"].as_array().unwrap().is_empty());
    let (ok, _, err) = keryx(&e, &["open", "https://example.com/x"]);
    assert!(!ok);
    assert!(err.contains("expected keryx://thread/<account>/<thread_id>"), "{err}");
    let (ok, _, err) = keryx(&e, &["task", "oxford:1", "x", "--estimate", "lots"]);
    assert!(!ok);
    assert!(err.contains("--estimate: try 2h, 90m or 1h30"), "{err}");
    let (ok, _, err) = keryx(&e, &["task", "oxford:1", "x", "--due", "wed"]);
    assert!(!ok);
    assert!(err.contains("ambiguous"), "{err}");
}
