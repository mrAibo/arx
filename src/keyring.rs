/// OS keyring integration for SSH passwords.
/// ponytail: uses system keyring via the `keyring` crate.
/// Falls back to SSH_PASSWORD env var if keyring unavailable.
use keyring::Entry;

const SERVICE: &str = "arx-ssh";

/// Get a password from the OS keyring for a given host.
/// Returns None if no entry exists or keyring is unavailable.
pub fn get_password(host: &str, user: &str) -> Option<String> {
    let entry = Entry::new(SERVICE, &format!("{user}@{host}")).ok()?;
    entry
        .get_password()
        .ok()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
}

/// Store a password in the OS keyring for a given host.
pub fn set_password(host: &str, user: &str, password: &str) -> Result<(), keyring::Error> {
    let entry = Entry::new(SERVICE, &format!("{user}@{host}"))?;
    entry.set_password(password)
}

/// Delete a stored password from the OS keyring.
pub fn delete_password(host: &str, user: &str) -> Result<(), keyring::Error> {
    let entry = Entry::new(SERVICE, &format!("{user}@{host}"))?;
    entry.delete_credential()
}

const WEBDAV_SERVICE: &str = "arx-webdav";

/// Resolve a WebDAV target secret. Keyring keyed by target `id` under a
/// dedicated service, with an `ARX_WEBDAV_<ID>_PASSWORD` env fallback (tests).
/// Returns None if no secret is configured.
pub fn webdav_secret(target_id: &str) -> Option<String> {
    let env_key = format!("ARX_WEBDAV_{}_PASSWORD", target_id.to_uppercase());
    if let Ok(v) = std::env::var(&env_key)
        && !v.is_empty()
    {
        return Some(v);
    }
    let entry = Entry::new(WEBDAV_SERVICE, target_id).ok()?;
    entry
        .get_password()
        .ok()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
}

/// Store a WebDAV target secret in the OS keyring.
#[allow(dead_code)]
pub fn set_webdav_password(target_id: &str, password: &str) -> Result<(), keyring::Error> {
    let entry = Entry::new(WEBDAV_SERVICE, target_id)?;
    entry.set_password(password)
}

/// Delete a stored WebDAV target secret.
#[allow(dead_code)]
pub fn delete_webdav_password(target_id: &str) -> Result<(), keyring::Error> {
    let entry = Entry::new(WEBDAV_SERVICE, target_id)?;
    entry.delete_credential()
}
