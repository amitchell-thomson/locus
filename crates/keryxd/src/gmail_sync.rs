//! Gmail job: refresh the access token and run one sync pass.

use keryx_core::config::{Account, Config};
use keryx_store::db::Db;
use keryx_store::gmail::{self, Client, ClientSecret, GmailError, Http};
use keryx_store::keychain;

pub const CLIENT_SERVICE: &str = "keryx-gmail-client";
pub const REFRESH_SERVICE: &str = "keryx-gmail-refresh";

pub fn sync_account(_cfg: &Config, db: &mut Db, acc: &Account) -> Result<(), GmailError> {
    let (Ok(client_json), Ok(refresh)) = (keychain::get(CLIENT_SERVICE), keychain::get(REFRESH_SERVICE)) else {
        tracing::debug!(account = %acc.id, "gmail not authorised yet (keryx auth gmail)");
        return Err(GmailError::NotAuthorised);
    };
    let client = ClientSecret::from_json(&client_json)?;
    let http = Http::new()?;
    let token = gmail::access_token(&http, &client, &refresh)?;
    let c = Client { t: &http, token };
    let t = std::time::Instant::now();
    let r = gmail::sync(db, &c, acc, crate::now())?;
    if r.full || r.added + r.updated + r.removed > 0 {
        tracing::info!(
            account = %acc.id, full = r.full, added = r.added, updated = r.updated, removed = r.removed,
            ms = t.elapsed().as_millis() as u64, "gmail sync"
        );
    }
    Ok(())
}
