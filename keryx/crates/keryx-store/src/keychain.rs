//! macOS Keychain access through the `security` tool (secrets never live in
//! the repo or the config). Services used by keryx: `keryx-api-token`,
//! `keryx-gmail-client`, `keryx-gmail-refresh`, `keryx-ntfy-topic`, and
//! kairos's `kairos-api-token`.

use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    #[error("no Keychain item {0:?} (see docs/setup.md)")]
    Missing(String),
    #[error("Keychain: {0}")]
    Other(String),
}

/// Read a generic password by service name.
pub fn get(service: &str) -> Result<String, KeychainError> {
    if let Ok(v) = std::env::var(env_name(service)) {
        return Ok(v);
    }
    let out = Command::new("security")
        .args(["find-generic-password", "-w", "-s", service])
        .output()
        .map_err(|e| KeychainError::Other(e.to_string()))?;
    if !out.status.success() {
        return Err(KeychainError::Missing(service.to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
}

/// Create or replace a generic password.
pub fn set(service: &str, value: &str) -> Result<(), KeychainError> {
    let user = std::env::var("USER").unwrap_or_else(|_| "keryx".into());
    let out = Command::new("security")
        .args(["add-generic-password", "-U", "-s", service, "-a", &user, "-w", value])
        .output()
        .map_err(|e| KeychainError::Other(e.to_string()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(KeychainError::Other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// Tests and CI can supply secrets as `KERYX_SECRET_<SERVICE>` instead
/// (e.g. `KERYX_SECRET_KERYX_API_TOKEN`).
pub fn env_name(service: &str) -> String {
    format!("KERYX_SECRET_{}", service.to_ascii_uppercase().replace('-', "_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_names() {
        assert_eq!(env_name("keryx-api-token"), "KERYX_SECRET_KERYX_API_TOKEN");
    }
}
