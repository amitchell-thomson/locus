//! `keryx auth gmail`: the OAuth loopback flow with PKCE (RFC 8252).

use anyhow::{Context, bail};
use keryx_store::gmail::{self, ClientSecret, Http};
use keryx_store::keychain;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

pub fn gmail() -> anyhow::Result<()> {
    let client_json =
        keychain::get("keryx-gmail-client").context("no OAuth client in the Keychain (docs/setup.md §2, step 5)")?;
    let client = ClientSecret::from_json(&client_json)?;

    let mut random = [0u8; 32];
    random[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    random[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    let (verifier, challenge) = gmail::pkce(&random);
    let state = uuid::Uuid::new_v4().simple().to_string();

    let listener = TcpListener::bind("127.0.0.1:0").context("opening a local port for Google's redirect")?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let url = gmail::auth_url(&client, &redirect, &challenge, &state);

    println!("Opening your browser to authorise keryx for Gmail (scope: gmail.modify).");
    println!("Google will say the app isn't verified: Advanced → Go to keryx.\n");
    println!("If the browser doesn't open, visit:\n{url}\n");
    let _ = std::process::Command::new("open").arg(&url).status();

    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(300);
    let code = loop {
        if Instant::now() > deadline {
            bail!("no answer from the browser within 5 minutes; run `keryx auth gmail` again");
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line)?;
                let result = gmail::code_from_request_line(&line, &state);
                let body = match &result {
                    Ok(_) => "keryx is authorised. You can close this tab.",
                    Err(_) => "keryx: that didn't work; see the terminal.",
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                match result {
                    Ok(code) => break code,
                    // Favicon requests and the like: keep waiting.
                    Err(e) if e.contains("no code") || e.contains("not an HTTP") => continue,
                    Err(e) => bail!("{e}"),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.into()),
        }
    };

    let http = Http::new()?;
    let refresh = gmail::exchange_code(&http, &client, &code, &verifier, &redirect)?;
    keychain::set("keryx-gmail-refresh", &refresh)?;
    gmail::access_token(&http, &client, &refresh).context("the new token doesn't work")?;
    println!("✓ Gmail authorised. keryxd will mirror it on its next pass (or run `keryxd --once`).");
    Ok(())
}
