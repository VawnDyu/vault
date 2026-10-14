// Put this file at: vault_app/rust/src/api/vault_api.rs
// and add `pub mod vault_api;` to vault_app/rust/src/api/mod.rs
//
// Every `pub fn` here becomes a Dart function after running
//   flutter_rust_bridge_codegen generate
// (normal functions become `Future`s in Dart, so slow work like Argon2
// never freezes the UI).

use std::sync::Mutex;
use vault_core::generator::{self, GeneratorOptions};
use vault_core::{calibrate_kdf, Vault, VaultError};

/// The unlocked vault lives here, inside Rust. Flutter never sees the key.
struct Unlocked {
    vault: Vault,
    path: String,
}

static STATE: Mutex<Option<Unlocked>> = Mutex::new(None);

fn with_unlocked<T>(f: impl FnOnce(&mut Unlocked) -> Result<T, String>) -> Result<T, String> {
    let mut guard = STATE.lock().map_err(|_| "internal error".to_string())?;
    let unlocked = guard.as_mut().ok_or_else(|| "vault is locked".to_string())?;
    f(unlocked)
}

/// What the entry list shows. The password is NOT included on purpose;
/// Flutter asks for it only when needed (see `get_entry_password`).
pub struct EntrySummary {
    pub id: u32,
    pub site: String,
    pub username: String,
    /// "social", "bank", ... or empty when uncategorized.
    pub category: String,
    /// True when this entry has a two-factor secret.
    pub has_totp: bool,
}

pub fn create_vault(path: String, password: String) -> Result<(), String> {
    if std::path::Path::new(&path).exists() {
        return Err("a vault already exists at this location".into());
    }
    // Tune the key-derivation strength to this device (about half a second).
    let kdf = calibrate_kdf(500);
    let vault = Vault::create(&password, kdf).map_err(|e| e.to_string())?;
    vault.save(&path).map_err(|e| e.to_string())?;
    *STATE.lock().map_err(|_| "internal error".to_string())? = Some(Unlocked { vault, path });
    Ok(())
}

pub fn unlock_vault(path: String, password: String) -> Result<(), String> {
    let vault = Vault::open_file(&path, &password).map_err(|e| e.to_string())?;
    *STATE.lock().map_err(|_| "internal error".to_string())? = Some(Unlocked { vault, path });
    Ok(())
}

/// Dropping the vault wipes the key from memory.
pub fn lock_vault() {
    if let Ok(mut guard) = STATE.lock() {
        *guard = None;
    }
}

pub fn is_unlocked() -> bool {
    STATE.lock().map(|g| g.is_some()).unwrap_or(false)
}

pub fn list_entries() -> Result<Vec<EntrySummary>, String> {
    with_unlocked(|u| {
        Ok(u.vault
            .entries()
            .iter()
            .map(|e| EntrySummary {
                id: e.id as u32,
                site: e.site.clone(),
                username: e.username.clone(),
                category: e.category.clone(),
                has_totp: e.totp.is_some(),
            })
            .collect())
    })
}

pub fn get_entry_password(id: u32) -> Result<String, String> {
    with_unlocked(|u| {
        u.vault
            .get_entry(id as u64)
            .map(|e| e.password.clone())
            .map_err(|e| e.to_string())
    })
}

/// Adds an entry and saves the vault file immediately.
pub fn add_entry(
    site: String,
    username: String,
    password: String,
    notes: String,
    category: String,
    totp: String,
) -> Result<u32, String> {
    with_unlocked(|u| {
        // Check the 2FA key first, so a typo never leaves a half-added entry.
        if !totp.trim().is_empty() {
            vault_core::totp::parse_totp_input(&totp).map_err(|e| e.to_string())?;
        }
        let id = u.vault.add_entry(&site, &username, &password, &notes);
        u.vault.set_category(id, &category).map_err(|e| e.to_string())?;
        u.vault.set_totp(id, &totp).map_err(|e| e.to_string())?;
        u.vault.save(&u.path).map_err(|e| e.to_string())?;
        Ok(id as u32)
    })
}

/// Full entry, used only when the user opens an entry to edit it.
pub struct EntryDetails {
    pub id: u32,
    pub site: String,
    pub username: String,
    pub password: String,
    pub notes: String,
    pub category: String,
    /// The 2FA setup key (empty if none). Shown when editing.
    pub totp: String,
}

pub fn get_entry(id: u32) -> Result<EntryDetails, String> {
    with_unlocked(|u| {
        let e = u.vault.get_entry(id as u64).map_err(|err| err.to_string())?;
        Ok(EntryDetails {
            id,
            site: e.site.clone(),
            username: e.username.clone(),
            password: e.password.clone(),
            notes: e.notes.clone(),
            category: e.category.clone(),
            totp: e.totp.as_ref().map(|t| t.to_input()).unwrap_or_default(),
        })
    })
}

/// Updates an entry and saves the vault file immediately.
pub fn update_entry(
    id: u32,
    site: String,
    username: String,
    password: String,
    notes: String,
    category: String,
    totp: String,
) -> Result<(), String> {
    with_unlocked(|u| {
        if !totp.trim().is_empty() {
            vault_core::totp::parse_totp_input(&totp).map_err(|e| e.to_string())?;
        }
        u.vault
            .update_entry(id as u64, &site, &username, &password, &notes)
            .map_err(|e| e.to_string())?;
        u.vault.set_category(id as u64, &category).map_err(|e| e.to_string())?;
        u.vault.set_totp(id as u64, &totp).map_err(|e| e.to_string())?;
        u.vault.save(&u.path).map_err(|e| e.to_string())
    })
}

/// Change the master password. The current password is checked first by
/// re-opening the vault file with it, so a stranger at an unlocked,
/// unattended screen can't change it without knowing it.
///
/// Returns a NEW recovery key if the vault had one (changing the password
/// replaces the data key, which invalidates the old recovery key).
pub fn change_master_password(
    old_password: String,
    new_password: String,
) -> Result<Option<String>, String> {
    with_unlocked(|u| {
        match Vault::open_file(&u.path, &old_password) {
            Ok(_) => {}
            Err(VaultError::WrongPasswordOrCorrupt) => {
                return Err("the current master password is incorrect".to_string());
            }
            Err(e) => return Err(e.to_string()),
        }
        let new_key = u.vault.change_password(&new_password).map_err(|e| e.to_string())?;
        u.vault.save(&u.path).map_err(|e| e.to_string())?;
        Ok(new_key)
    })
}

// ---------- Recovery key ----------

pub fn has_recovery_key() -> Result<bool, String> {
    with_unlocked(|u| Ok(u.vault.has_recovery_key()))
}

/// Creates (or replaces) the recovery key and returns it for display.
/// It is not stored anywhere, so this is the only time it can be shown.
///
/// The master password is required again, because a recovery key is as
/// powerful as the password itself.
pub fn create_recovery_key(password: String) -> Result<String, String> {
    with_unlocked(|u| {
        match Vault::open_file(&u.path, &password) {
            Ok(_) => {}
            Err(VaultError::WrongPasswordOrCorrupt) => {
                return Err("the master password is incorrect".to_string());
            }
            Err(e) => return Err(e.to_string()),
        }
        let key = u.vault.set_recovery_key().map_err(|e| e.to_string())?;
        u.vault.save(&u.path).map_err(|e| e.to_string())?;
        Ok(key)
    })
}

/// "Forgot my password" flow: unlock with the recovery key, set a new master
/// password, and stay unlocked. Works while the vault is locked.
/// Returns the NEW recovery key to show the user.
pub fn recover_with_key(
    path: String,
    recovery_key: String,
    new_password: String,
) -> Result<Option<String>, String> {
    let mut vault = Vault::open_file_with_recovery(&path, &recovery_key).map_err(|e| match e {
        VaultError::WrongPasswordOrCorrupt => {
            "wrong recovery key, or the vault file is damaged".to_string()
        }
        VaultError::NoRecoveryKey => "this vault has no recovery key".to_string(),
        VaultError::InvalidRecoveryKey => "that doesn't look like a recovery key".to_string(),
        other => other.to_string(),
    })?;
    let new_key = vault.change_password(&new_password).map_err(|e| e.to_string())?;
    vault.save(&path).map_err(|e| e.to_string())?;
    *STATE.lock().map_err(|_| "internal error".to_string())? = Some(Unlocked { vault, path });
    Ok(new_key)
}

// ---------- Backup ----------

/// The vault as encrypted bytes, for saving a backup file. These bytes are
/// ciphertext, so it is safe to pass them through Dart to the file system.
pub fn export_backup_bytes() -> Result<Vec<u8>, String> {
    with_unlocked(|u| u.vault.to_bytes().map_err(|e| e.to_string()))
}

/// Restore from a backup: checks the backup opens with `password`, keeps the
/// current vault file as a ".vault.bak" file, installs the backup, unlocks it.
pub fn import_backup(path: String, bytes: Vec<u8>, password: String) -> Result<(), String> {
    let vault = Vault::open(&bytes, &password).map_err(|e| match e {
        VaultError::WrongPasswordOrCorrupt => {
            "wrong password for this backup, or the file is damaged".to_string()
        }
        other => other.to_string(),
    })?;
    let p = std::path::Path::new(&path);
    if p.exists() {
        std::fs::copy(p, p.with_extension("vault.bak")).map_err(|e| e.to_string())?;
    }
    vault.save(&path).map_err(|e| e.to_string())?;
    *STATE.lock().map_err(|_| "internal error".to_string())? = Some(Unlocked { vault, path });
    Ok(())
}

// ---------- Quick unlock (fingerprint / face) ----------

/// The vault's data key as text. The app stores it ONLY in the phone's
/// biometric-protected storage (Android Keystore / iOS Keychain). The master
/// password itself is never stored.
pub fn export_quick_unlock_key() -> Result<String, String> {
    with_unlocked(|u| Ok(u.vault.quick_unlock_key()))
}

/// Unlock with the key from `export_quick_unlock_key`.
pub fn unlock_with_quick_key(path: String, quick_key: String) -> Result<(), String> {
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let vault = Vault::open_with_quick_key(&bytes, &quick_key).map_err(|e| e.to_string())?;
    *STATE.lock().map_err(|_| "internal error".to_string())? = Some(Unlocked { vault, path });
    Ok(())
}

/// Result of a CSV import, shown to the user.
pub struct ImportSummary {
    pub added: u32,
    pub duplicates: u32,
    pub skipped: u32,
}

/// Adds the logins found in a CSV export (Chrome, Edge, Firefox, Bitwarden...).
/// Entries already in the vault are skipped. A safety copy of the current
/// vault file is made first, ending in ".before-import".
pub fn import_csv(bytes: Vec<u8>) -> Result<ImportSummary, String> {
    with_unlocked(|u| {
        let report = u.vault.import_csv(&bytes).map_err(|e| e.to_string())?;
        if report.added > 0 {
            let safety = std::path::Path::new(&u.path).with_extension("vault.before-import");
            std::fs::copy(&u.path, safety).map_err(|e| e.to_string())?;
            u.vault.save(&u.path).map_err(|e| e.to_string())?;
        }
        Ok(ImportSummary {
            added: report.added,
            duplicates: report.duplicates,
            skipped: report.skipped,
        })
    })
}

/// One problem found by the password health check.
pub struct HealthIssue {
    pub id: u32,
    pub site: String,
    pub username: String,
    pub category: String,
    /// Either "reused" or "weak".
    pub kind: String,
    /// For weak passwords: a short explanation. Empty for reused ones.
    pub detail: String,
    /// For reused passwords: entries with the same password share a group
    /// number (1, 2, ...). Always 0 for weak passwords.
    pub group: u32,
}

/// The overall result of the password health check.
pub struct HealthSummary {
    pub total: u32,
    pub healthy: u32,
    pub issues: Vec<HealthIssue>,
}

fn health_issue(
    vault: &Vault,
    id: u64,
    kind: &str,
    detail: &str,
    group: u32,
) -> Result<HealthIssue, String> {
    let e = vault.get_entry(id).map_err(|err| err.to_string())?;
    Ok(HealthIssue {
        id: id as u32,
        site: e.site.clone(),
        username: e.username.clone(),
        category: e.category.clone(),
        kind: kind.to_string(),
        detail: detail.to_string(),
        group,
    })
}

/// Finds reused and weak passwords. Passwords are compared inside Rust; only
/// the list of problems (never a password) is sent to the screen.
pub fn password_health() -> Result<HealthSummary, String> {
    with_unlocked(|u| {
        let report = u.vault.health_report();
        let mut issues = Vec::new();
        for (index, group) in report.reused_groups.iter().enumerate() {
            for &id in group {
                issues.push(health_issue(&u.vault, id, "reused", "", index as u32 + 1)?);
            }
        }
        for (id, reason) in &report.weak {
            issues.push(health_issue(&u.vault, *id, "weak", reason.describe(), 0)?);
        }
        let unhealthy = report.unhealthy_ids().len();
        Ok(HealthSummary {
            total: report.total as u32,
            healthy: report.total.saturating_sub(unhealthy) as u32,
            issues,
        })
    })
}

/// A current two-factor code for one entry.
pub struct TotpView {
    pub id: u32,
    pub site: String,
    pub username: String,
    pub category: String,
    pub code: String,
    pub seconds_remaining: u32,
    pub period: u32,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Current codes for every entry that has a 2FA secret, sorted by site.
/// Codes depend on the device clock, so it must be set correctly.
pub fn totp_codes() -> Result<Vec<TotpView>, String> {
    with_unlocked(|u| {
        let now = unix_now();
        let mut out = Vec::new();
        for e in u.vault.entries() {
            if e.totp.is_none() {
                continue;
            }
            if let Some(c) = u.vault.totp_code(e.id, now).map_err(|err| err.to_string())? {
                out.push(TotpView {
                    id: e.id as u32,
                    site: e.site.clone(),
                    username: e.username.clone(),
                    category: e.category.clone(),
                    code: c.code,
                    seconds_remaining: c.seconds_remaining,
                    period: c.period,
                });
            }
        }
        out.sort_by(|a, b| a.site.to_lowercase().cmp(&b.site.to_lowercase()));
        Ok(out)
    })
}

/// What a sync merge did.
pub struct MergeSummary {
    /// True when the other file uses a different key, so its own master
    /// password is needed. Nothing was merged in that case.
    pub needs_password: bool,
    pub added: u32,
    pub updated: u32,
    pub deleted: u32,
    pub unchanged: u32,
}

/// Merge another device's copy of the vault into this one.
///
/// Copies of the same vault share a data key, so this first tries to open the
/// other file with the key already in use (no password needed). If that fails
/// and `password` is empty, it reports `needs_password`; otherwise the file is
/// opened with the given password. A safety copy of the current vault is kept
/// next to it, ending in ".before-sync", whenever something changes.
pub fn merge_vault_bytes(bytes: Vec<u8>, password: String) -> Result<MergeSummary, String> {
    with_unlocked(|u| {
        let other = match Vault::open_with_quick_key(&bytes, &u.vault.quick_unlock_key()) {
            Ok(v) => v,
            Err(VaultError::WrongPasswordOrCorrupt) if password.is_empty() => {
                return Ok(MergeSummary {
                    needs_password: true,
                    added: 0,
                    updated: 0,
                    deleted: 0,
                    unchanged: 0,
                });
            }
            Err(VaultError::WrongPasswordOrCorrupt) => Vault::open(&bytes, &password)
                .map_err(|e| match e {
                    VaultError::WrongPasswordOrCorrupt => {
                        "wrong password for that file, or the file is damaged".to_string()
                    }
                    other => other.to_string(),
                })?,
            Err(e) => return Err(e.to_string()),
        };

        let report = u.vault.merge_from(&other);
        if report.added + report.updated + report.deleted > 0 {
            let safety = std::path::Path::new(&u.path).with_extension("vault.before-sync");
            std::fs::copy(&u.path, safety).map_err(|e| e.to_string())?;
            u.vault.save(&u.path).map_err(|e| e.to_string())?;
        }
        Ok(MergeSummary {
            needs_password: false,
            added: report.added,
            updated: report.updated,
            deleted: report.deleted,
            unchanged: report.unchanged,
        })
    })
}

pub fn delete_entry(id: u32) -> Result<(), String> {
    with_unlocked(|u| {
        u.vault.delete_entry(id as u64).map_err(|e| e.to_string())?;
        u.vault.save(&u.path).map_err(|e| e.to_string())
    })
}

pub fn generate_password(
    length: u32,
    symbols: bool,
    exclude_ambiguous: bool,
) -> Result<String, String> {
    let opts = GeneratorOptions {
        length: length as usize,
        symbols,
        exclude_ambiguous,
        ..Default::default()
    };
    generator::generate(&opts).map_err(|e| e.to_string())
}