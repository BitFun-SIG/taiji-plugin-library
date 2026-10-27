//! Qoder machine-id sourcing (CLI-same origin).
//!
//! The gateway validates the signature against the id the Qoder CN CLI used, so
//! the seam reads the CLI's own file (`~/.qoder-cn/.auth/machine_id`) rather
//! than minting an independent one. A fresh UUID v4 is persisted to that same
//! location on first use, so the host and the CLI stay in lockstep.

use std::path::PathBuf;

/// Returns the machine id, creating and persisting a fresh UUID v4 on first use.
///
/// Read/write failures are non-fatal: the generated value is still returned so
/// callers can sign a request, and a later run retries persistence.
pub fn qoder_machine_id() -> String {
    let Some(path) = machine_id_path() else {
        return new_uuid_v4();
    };
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let generated = new_uuid_v4();
    persist(&path, &generated);
    generated
}

/// Absolute path of the machine-id file: `~/.qoder-cn/.auth/machine_id`.
///
/// This is the CLI's own location; reading and writing it keeps the seam's
/// signature byte-identical to the client the gateway expects.
pub(crate) fn machine_id_path() -> Option<PathBuf> {
    let home = home_dir()?;
    Some(home.join(LEGACY_AUTH_DIR).join("machine_id"))
}

/// The CLI's auth directory relative to the user home.
const LEGACY_AUTH_DIR: &str = ".qoder-cn/.auth";

/// Resolves the user home directory without pulling an extra dependency.
fn home_dir() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("HOME") {
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    if let Some(value) = std::env::var_os("USERPROFILE") {
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    let drive = std::env::var_os("HOMEDRIVE")?;
    let path = std::env::var_os("HOMEPATH")?;
    if drive.is_empty() || path.is_empty() {
        return None;
    }
    let mut home = PathBuf::from(drive);
    home.push(path);
    Some(home)
}

/// Best-effort persist; failure keeps the in-memory value usable.
fn persist(path: &PathBuf, value: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let _ = std::fs::write(path, value);
}

/// Produces a UUID v4 string (RFC 4122 layout) from 16 random bytes.
fn new_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    fill_random(&mut bytes);
    // Version 4 and variant 1 markers.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

/// Fills `bytes` with entropy. Uses the CSPRNG when the auth feature is on and
/// a time/pid-derived mix otherwise, so the function is always available.
#[cfg(feature = "subscription-auth")]
fn fill_random(bytes: &mut [u8]) {
    if getrandom::getrandom(bytes).is_err() {
        fill_random_fallback(bytes);
    }
}

#[cfg(not(feature = "subscription-auth"))]
fn fill_random(bytes: &mut [u8]) {
    fill_random_fallback(bytes);
}

/// Non-cryptographic fallback used only when no CSPRNG is compiled in.
fn fill_random_fallback(bytes: &mut [u8]) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    let mut seed = hasher.finish();
    for slot in bytes.iter_mut() {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *slot = (seed >> 33) as u8;
    }
}
