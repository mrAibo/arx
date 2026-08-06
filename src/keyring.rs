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
