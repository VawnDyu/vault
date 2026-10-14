//! vault_core: offline encrypted vault (Argon2id + XChaCha20-Poly1305).
//!
//! Design ("envelope encryption"):
//!   * The vault data is encrypted with a random 32-byte data key (DEK).
//!   * The DEK is stored in the file "wrapped" (encrypted) in up to two ways:
//!       1. under a key derived from the master password (Argon2id), and
//!       2. optionally under the recovery key (32 random bytes shown to the
//!          user once).
//!   * Changing the master password creates a NEW DEK, so anyone who knew
//!     the old password can't decrypt the new file.
//!
//! File layout (integers little-endian), HEADER_LEN = 202 bytes:
//!   0   magic "VALT" (4)
//!   4   format version = 2 (1)
//!   5   argon2 m_cost KiB (4) | t_cost (4) | p_cost (4)
//!   17  salt (16)
//!   33  password-wrap: nonce (24) + wrapped DEK (32 + 16 tag) = 72
//!   105 recovery flag (1): 0 = none, 1 = present
//!   106 recovery-wrap: nonce (24) + wrapped DEK (48) = 72 (zeros if none)
//!   178 data nonce (24)
//!   202 ciphertext + auth tag (rest)
//! The whole header is authenticated (used as AAD for the data), so changing
//! any header byte makes decryption fail.

pub mod generator;
pub mod health;
pub mod import;
pub mod sync;
pub mod totp;

pub use import::ImportReport;
pub use sync::MergeReport;
pub use totp::{TotpCode, TotpConfig};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    Key, XChaCha20Poly1305, XNonce,
};
use data_encoding::BASE32_NOPAD;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::{fmt, fs, io, path::Path};
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"VALT";
const FORMAT_VERSION: u8 = 2;

const OFF_KDF: usize = 5;
const OFF_SALT: usize = 17;
const OFF_PW_WRAP: usize = 33;
const OFF_REC_FLAG: usize = 105;
const OFF_REC_WRAP: usize = 106;
const OFF_DATA_NONCE: usize = 178;
const HEADER_LEN: usize = 202;
const WRAP_LEN: usize = 72; // nonce (24) + wrapped key (48)
const TAG_LEN: usize = 16;

// Context labels so a wrap made for one purpose can't be used for the other.
const CTX_PW: &[u8] = b"vault-dek-password";
const CTX_REC: &[u8] = b"vault-dek-recovery";

// ---------- Errors ----------

#[derive(Debug)]
pub enum VaultError {
    /// Wrong password / recovery key, OR the file was modified/corrupted.
    WrongPasswordOrCorrupt,
    /// Not a vault file, or too short/malformed.
    InvalidFormat,
    UnsupportedVersion(u8),
    /// This vault has no recovery key set up.
    NoRecoveryKey,
    /// The text entered doesn't look like a recovery key.
    InvalidRecoveryKey,
    EntryNotFound,
    Io(io::Error),
    Internal(String),
    /// A problem with a file being imported (message is meant for the user).
    Import(String),
    /// A problem with a two-factor setup key (message is meant for the user).
    Totp(String),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::WrongPasswordOrCorrupt => {
                write!(f, "wrong master password, or the vault file is damaged")
            }
            VaultError::InvalidFormat => write!(f, "not a valid vault file"),
            VaultError::UnsupportedVersion(v) => write!(f, "unsupported vault version {v}"),
            VaultError::NoRecoveryKey => write!(f, "this vault has no recovery key"),
            VaultError::InvalidRecoveryKey => write!(f, "that is not a valid recovery key"),
            VaultError::EntryNotFound => write!(f, "entry not found"),
            VaultError::Io(e) => write!(f, "file error: {e}"),
            VaultError::Internal(s) => write!(f, "internal error: {s}"),
            VaultError::Import(s) => write!(f, "{s}"),
            VaultError::Totp(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<io::Error> for VaultError {
    fn from(e: io::Error) -> Self {
        VaultError::Io(e)
    }
}

// ---------- Settings for the key derivation ----------

#[derive(Debug, Clone, Copy)]
pub struct KdfParams {
    pub m_cost_kib: u32, // memory in KiB
    pub t_cost: u32,     // iterations
    pub p_cost: u32,     // parallel lanes
}

impl Default for KdfParams {
    fn default() -> Self {
        // 64 MiB, 3 passes, 1 lane. Raise these as devices allow.
        KdfParams { m_cost_kib: 64 * 1024, t_cost: 3, p_cost: 1 }
    }
}

impl KdfParams {
    /// Reject absurd values from a (possibly malicious) file so it can't
    /// make us allocate gigabytes of memory.
    fn validate(&self) -> Result<(), VaultError> {
        if self.m_cost_kib > 1024 * 1024 || self.t_cost > 20 || self.p_cost > 16 {
            return Err(VaultError::InvalidFormat);
        }
        Ok(())
    }
}

/// Pick key-derivation settings for THIS device: always 64 MiB of memory (that
/// is what makes GPU/ASIC guessing expensive), and as many passes (3 to 10) as
/// fit in roughly `target_ms` milliseconds. Fast computers get more passes, so
/// guessing costs an attacker more; slow phones stay at the safe minimum.
///
/// The chosen numbers are stored in the vault header, so a vault created on
/// one device still opens on another (just faster or slower there).
/// Call this once when CREATING a vault, not when opening one.
pub fn calibrate_kdf(target_ms: u64) -> KdfParams {
    let base = KdfParams::default();
    let probe = KdfParams { t_cost: 1, ..base };
    let salt = [7u8; 16]; // any value; only the timing matters
    let start = std::time::Instant::now();
    if derive_key("calibration", &salt, &probe).is_err() {
        return base;
    }
    let one_pass_ms = (start.elapsed().as_millis() as u64).max(1);
    let passes = (target_ms / one_pass_ms).clamp(3, 10) as u32;
    KdfParams { t_cost: passes, ..base }
}

// ---------- Vault data ----------

#[derive(Serialize, Deserialize, Clone)]
pub struct Entry {
    pub id: u64,
    pub site: String,
    pub username: String,
    pub password: String,
    pub notes: String,
    /// Short lowercase label such as "social" or "bank". Empty = uncategorized.
    /// `default` keeps vaults saved before categories existed opening fine.
    #[serde(default)]
    pub category: String,
    /// Two-factor (TOTP) settings, if this account has them.
    #[serde(default)]
    pub totp: Option<TotpConfig>,
    /// Unique id that is the same on every device, used to match entries up
    /// when syncing. (`id` above is only a number local to this device.)
    #[serde(default)]
    pub uid: String,
    /// When this entry last changed (milliseconds since 1970). The newest
    /// change wins when two devices are merged.
    #[serde(default)]
    pub updated: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct VaultData {
    entries: Vec<Entry>,
    next_id: u64,
    /// Records of deleted entries, so a later sync does not bring them back.
    #[serde(default)]
    deleted: Vec<Tombstone>,
}

/// A note that an entry was deleted (see `sync`).
#[derive(Serialize, Deserialize, Clone)]
struct Tombstone {
    uid: String,
    /// When it was deleted (milliseconds since 1970).
    at: u64,
}

/// Deletion records older than this are forgotten (one year).
const TOMBSTONE_KEEP_MS: u64 = 365 * 24 * 60 * 60 * 1000;

/// Milliseconds since 1970-01-01 UTC.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A fresh random identity for a new entry.
fn new_uid() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    data_encoding::HEXLOWER.encode(&bytes)
}

impl VaultData {
    /// Entries saved before syncing existed have no `uid`. Give each one an id
    /// worked out from its content, so the same entry gets the same id on
    /// every device that opens an old copy (identical entries are numbered).
    fn normalize_uids(&mut self) {
        use sha2::{Digest, Sha256};
        let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for e in self.entries.iter_mut() {
            if !e.uid.is_empty() {
                continue;
            }
            let mut hasher = Sha256::new();
            for part in [e.site.as_str(), e.username.as_str(), e.password.as_str()] {
                hasher.update(part.as_bytes());
                hasher.update([0u8]);
            }
            let digest = hasher.finalize();
            let base = data_encoding::HEXLOWER.encode(&digest[..12]);
            let n = seen.entry(base.clone()).or_insert(0);
            e.uid = format!("legacy-{base}-{n}");
            *n += 1;
        }
    }
}

/// A data key encrypted under some other key.
#[derive(Clone)]
struct Wrapped {
    nonce: [u8; 24],
    bytes: [u8; 48],
}

/// An unlocked vault. Holds the data key in memory (wiped when dropped).
pub struct Vault {
    data: VaultData,
    dek: Zeroizing<[u8; 32]>,
    salt: [u8; 16],
    kdf: KdfParams,
    pw_wrap: Wrapped,
    rec_wrap: Option<Wrapped>,
}

/// What we can read from a file without any secret.
struct Header {
    kdf: KdfParams,
    salt: [u8; 16],
    pw_wrap: Wrapped,
    rec_wrap: Option<Wrapped>,
}

// ---------- Low-level helpers ----------

fn derive_key(
    password: &str,
    salt: &[u8; 16],
    p: &KdfParams,
) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    p.validate()?;
    let params = Params::new(p.m_cost_kib, p.t_cost, p.p_cost, Some(32))
        .map_err(|_| VaultError::InvalidFormat)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key[..])
        .map_err(|e| VaultError::Internal(e.to_string()))?;
    Ok(key)
}

fn random_key() -> Zeroizing<[u8; 32]> {
    let mut k = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(&mut k[..]);
    k
}

/// Encrypt the data key `dek` under `kek`.
fn wrap_key(kek: &[u8; 32], dek: &[u8; 32], context: &[u8]) -> Result<Wrapped, VaultError> {
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&kek[..]));
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: &dek[..], aad: context })
        .map_err(|_| VaultError::Internal("key wrapping failed".into()))?;
    let bytes: [u8; 48] = ct
        .try_into()
        .map_err(|_| VaultError::Internal("unexpected wrapped key size".into()))?;
    Ok(Wrapped { nonce, bytes })
}

/// Decrypt a wrapped data key. Fails if `kek` is wrong or the data was changed.
fn unwrap_key(
    kek: &[u8; 32],
    w: &Wrapped,
    context: &[u8],
) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&kek[..]));
    let plain = Zeroizing::new(
        cipher
            .decrypt(XNonce::from_slice(&w.nonce), Payload { msg: &w.bytes[..], aad: context })
            .map_err(|_| VaultError::WrongPasswordOrCorrupt)?,
    );
    if plain.len() != 32 {
        return Err(VaultError::InvalidFormat);
    }
    let mut dek = Zeroizing::new([0u8; 32]);
    dek.copy_from_slice(&plain[..]);
    Ok(dek)
}

fn read_wrap(b: &[u8]) -> Wrapped {
    let mut nonce = [0u8; 24];
    nonce.copy_from_slice(&b[..24]);
    let mut bytes = [0u8; 48];
    bytes.copy_from_slice(&b[24..WRAP_LEN]);
    Wrapped { nonce, bytes }
}

fn write_wrap(out: &mut Vec<u8>, w: &Wrapped) {
    out.extend_from_slice(&w.nonce);
    out.extend_from_slice(&w.bytes);
}

fn parse_header(bytes: &[u8]) -> Result<Header, VaultError> {
    if bytes.len() < HEADER_LEN + TAG_LEN || &bytes[0..4] != MAGIC {
        return Err(VaultError::InvalidFormat);
    }
    let version = bytes[4];
    if version != FORMAT_VERSION {
        return Err(VaultError::UnsupportedVersion(version));
    }
    let read_u32 = |a: usize| u32::from_le_bytes(bytes[a..a + 4].try_into().unwrap());
    let kdf = KdfParams {
        m_cost_kib: read_u32(OFF_KDF),
        t_cost: read_u32(OFF_KDF + 4),
        p_cost: read_u32(OFF_KDF + 8),
    };
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&bytes[OFF_SALT..OFF_SALT + 16]);
    let pw_wrap = read_wrap(&bytes[OFF_PW_WRAP..OFF_PW_WRAP + WRAP_LEN]);
    let rec_wrap = match bytes[OFF_REC_FLAG] {
        0 => None,
        1 => Some(read_wrap(&bytes[OFF_REC_WRAP..OFF_REC_WRAP + WRAP_LEN])),
        _ => return Err(VaultError::InvalidFormat),
    };
    Ok(Header { kdf, salt, pw_wrap, rec_wrap })
}

fn decrypt_data(bytes: &[u8], dek: &[u8; 32]) -> Result<VaultData, VaultError> {
    let nonce = &bytes[OFF_DATA_NONCE..HEADER_LEN];
    let header = &bytes[..HEADER_LEN];
    let ciphertext = &bytes[HEADER_LEN..];
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&dek[..]));
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ciphertext, aad: header })
            .map_err(|_| VaultError::WrongPasswordOrCorrupt)?,
    );
    let mut data: VaultData =
        serde_json::from_slice(&plaintext[..]).map_err(|_| VaultError::InvalidFormat)?;
    data.normalize_uids();
    Ok(data)
}

/// 32 random bytes -> "ABCD-EFGH-..." (base32 in groups of four).
fn format_recovery_key(rk: &[u8; 32]) -> String {
    let b32 = BASE32_NOPAD.encode(&rk[..]);
    b32.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("-")
}

/// Forgiving parser: ignores case, dashes and spaces.
fn parse_recovery_key(s: &str) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    let raw = Zeroizing::new(
        BASE32_NOPAD
            .decode(cleaned.as_bytes())
            .map_err(|_| VaultError::InvalidRecoveryKey)?,
    );
    if raw.len() != 32 {
        return Err(VaultError::InvalidRecoveryKey);
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&raw[..]);
    Ok(key)
}

// ---------- The vault ----------

impl Vault {
    /// Create a new, empty vault protected by `password`.
    pub fn create(password: &str, kdf: KdfParams) -> Result<Vault, VaultError> {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let kek = derive_key(password, &salt, &kdf)?;
        let dek = random_key();
        let pw_wrap = wrap_key(&kek, &dek, CTX_PW)?;
        Ok(Vault { data: VaultData::default(), dek, salt, kdf, pw_wrap, rec_wrap: None })
    }

    /// Unlock a vault from its raw bytes using the master password.
    pub fn open(bytes: &[u8], password: &str) -> Result<Vault, VaultError> {
        let h = parse_header(bytes)?;
        let kek = derive_key(password, &h.salt, &h.kdf)?;
        let dek = unwrap_key(&kek, &h.pw_wrap, CTX_PW)?;
        let data = decrypt_data(bytes, &dek)?;
        Ok(Vault { data, dek, salt: h.salt, kdf: h.kdf, pw_wrap: h.pw_wrap, rec_wrap: h.rec_wrap })
    }

    /// Unlock a vault from its raw bytes using the recovery key.
    /// Afterwards, call `change_password` to set a new master password.
    pub fn open_with_recovery(bytes: &[u8], recovery_key: &str) -> Result<Vault, VaultError> {
        let h = parse_header(bytes)?;
        let rec = h.rec_wrap.clone().ok_or(VaultError::NoRecoveryKey)?;
        let rk = parse_recovery_key(recovery_key)?;
        let dek = unwrap_key(&rk, &rec, CTX_REC)?;
        let data = decrypt_data(bytes, &dek)?;
        Ok(Vault { data, dek, salt: h.salt, kdf: h.kdf, pw_wrap: h.pw_wrap, rec_wrap: h.rec_wrap })
    }

    pub fn open_file<P: AsRef<Path>>(path: P, password: &str) -> Result<Vault, VaultError> {
        let bytes = fs::read(path)?;
        Vault::open(&bytes, password)
    }

    /// The data key as 64 hex characters, for "quick unlock" (fingerprint or
    /// face). The caller MUST keep it only in hardware-protected storage that
    /// needs biometric authentication to read (Android Keystore, iOS
    /// Keychain). It stops working whenever the data key changes, which
    /// happens when the master password is changed.
    pub fn quick_unlock_key(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.dek[..])
    }

    /// Unlock with the key from `quick_unlock_key`. No password and no slow
    /// key derivation involved.
    pub fn open_with_quick_key(bytes: &[u8], quick_key: &str) -> Result<Vault, VaultError> {
        let h = parse_header(bytes)?;
        let raw = Zeroizing::new(
            data_encoding::HEXLOWER
                .decode(quick_key.trim().as_bytes())
                .map_err(|_| VaultError::WrongPasswordOrCorrupt)?,
        );
        if raw.len() != 32 {
            return Err(VaultError::WrongPasswordOrCorrupt);
        }
        let mut dek = Zeroizing::new([0u8; 32]);
        dek.copy_from_slice(&raw[..]);
        let data = decrypt_data(bytes, &dek)?;
        Ok(Vault { data, dek, salt: h.salt, kdf: h.kdf, pw_wrap: h.pw_wrap, rec_wrap: h.rec_wrap })
    }

    pub fn open_file_with_recovery<P: AsRef<Path>>(
        path: P,
        recovery_key: &str,
    ) -> Result<Vault, VaultError> {
        let bytes = fs::read(path)?;
        Vault::open_with_recovery(&bytes, recovery_key)
    }

    /// Encrypt the vault into bytes. A fresh random nonce is used every time.
    pub fn to_bytes(&self) -> Result<Vec<u8>, VaultError> {
        let mut out = Vec::with_capacity(HEADER_LEN + 256);
        out.extend_from_slice(MAGIC);
        out.push(FORMAT_VERSION);
        out.extend_from_slice(&self.kdf.m_cost_kib.to_le_bytes());
        out.extend_from_slice(&self.kdf.t_cost.to_le_bytes());
        out.extend_from_slice(&self.kdf.p_cost.to_le_bytes());
        out.extend_from_slice(&self.salt);
        write_wrap(&mut out, &self.pw_wrap);
        match &self.rec_wrap {
            Some(w) => {
                out.push(1);
                write_wrap(&mut out, w);
            }
            None => {
                out.push(0);
                out.extend_from_slice(&[0u8; WRAP_LEN]);
            }
        }
        let mut nonce = [0u8; 24];
        OsRng.fill_bytes(&mut nonce);
        out.extend_from_slice(&nonce);
        debug_assert_eq!(out.len(), HEADER_LEN);

        let plaintext = Zeroizing::new(
            serde_json::to_vec(&self.data).map_err(|e| VaultError::Internal(e.to_string()))?,
        );
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.dek[..]));
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &plaintext[..], aad: &out })
            .map_err(|_| VaultError::Internal("encryption failed".into()))?;
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    /// Save safely: write a temp file, then replace the real file, so a crash
    /// mid-save can't leave you with a half-written vault.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), VaultError> {
        let path = path.as_ref();
        let tmp = path.with_extension("vault.tmp");
        fs::write(&tmp, self.to_bytes()?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    // ----- recovery key -----

    pub fn has_recovery_key(&self) -> bool {
        self.rec_wrap.is_some()
    }

    /// Create (or replace) the recovery key. Returns it as text to show the
    /// user ONCE; it is not stored anywhere. Call `save` afterwards.
    pub fn set_recovery_key(&mut self) -> Result<String, VaultError> {
        let rk = random_key();
        self.rec_wrap = Some(wrap_key(&rk, &self.dek, CTX_REC)?);
        Ok(format_recovery_key(&rk))
    }

    // ----- master password -----

    /// Re-protect the vault with a new master password. This also creates a
    /// brand-new data key and salt. If the vault had a recovery key, a NEW
    /// recovery key is created (the old one stops working) and returned.
    /// Call `save` afterwards to write the file.
    pub fn change_password(&mut self, new_password: &str) -> Result<Option<String>, VaultError> {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let kek = derive_key(new_password, &salt, &self.kdf)?;
        let new_dek = random_key();
        let pw_wrap = wrap_key(&kek, &new_dek, CTX_PW)?;

        let had_recovery = self.rec_wrap.is_some();
        self.salt = salt;
        self.dek = new_dek; // the old data key is wiped when it is dropped
        self.pw_wrap = pw_wrap;
        self.rec_wrap = None;
        if had_recovery {
            Ok(Some(self.set_recovery_key()?))
        } else {
            Ok(None)
        }
    }

    // ----- entries -----

    pub fn entries(&self) -> &[Entry] {
        &self.data.entries
    }

    pub fn get_entry(&self, id: u64) -> Result<&Entry, VaultError> {
        self.data.entries.iter().find(|e| e.id == id).ok_or(VaultError::EntryNotFound)
    }

    pub fn add_entry(&mut self, site: &str, username: &str, password: &str, notes: &str) -> u64 {
        let id = self.data.next_id;
        self.data.next_id += 1;
        self.data.entries.push(Entry {
            id,
            site: site.into(),
            username: username.into(),
            password: password.into(),
            notes: notes.into(),
            category: String::new(),
            totp: None,
            uid: new_uid(),
            updated: now_ms(),
        });
        id
    }

    /// Set an entry's category (for example "social" or "bank"). Stored as
    /// lowercase letters, digits, `_` or `-`, up to 32 characters. An empty
    /// string clears the category.
    pub fn set_category(&mut self, id: u64, category: &str) -> Result<(), VaultError> {
        let c = category.trim().to_lowercase();
        if c.len() > 32 || !c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
            return Err(VaultError::Internal("invalid category".into()));
        }
        let e = self
            .data
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(VaultError::EntryNotFound)?;
        e.category = c;
        e.updated = now_ms();
        Ok(())
    }

    pub fn update_entry(
        &mut self,
        id: u64,
        site: &str,
        username: &str,
        password: &str,
        notes: &str,
    ) -> Result<(), VaultError> {
        let e = self
            .data
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(VaultError::EntryNotFound)?;
        e.site = site.into();
        e.username = username.into();
        e.password = password.into();
        e.notes = notes.into();
        e.updated = now_ms();
        Ok(())
    }

    pub fn delete_entry(&mut self, id: u64) -> Result<(), VaultError> {
        let pos = self
            .data
            .entries
            .iter()
            .position(|e| e.id == id)
            .ok_or(VaultError::EntryNotFound)?;
        let removed = self.data.entries.remove(pos);
        let now = now_ms();
        self.data
            .deleted
            .retain(|t| now.saturating_sub(t.at) < TOMBSTONE_KEEP_MS && t.uid != removed.uid);
        self.data.deleted.push(Tombstone { uid: removed.uid, at: now });
        Ok(())
    }
}

// ---------- Tests (run with: cargo test) ----------

#[cfg(test)]
mod tests {
    use super::*;

    // Cheap settings so tests run fast. Never use these for real vaults.
    const FAST: KdfParams = KdfParams { m_cost_kib: 8, t_cost: 1, p_cost: 1 };

    fn sample_vault() -> Vault {
        let mut v = Vault::create("master-pw", FAST).unwrap();
        v.add_entry("example.com", "me", "hunter2", "note");
        v
    }

    #[test]
    fn roundtrip() {
        let bytes = sample_vault().to_bytes().unwrap();
        let v = Vault::open(&bytes, "master-pw").unwrap();
        assert_eq!(v.entries().len(), 1);
        assert_eq!(v.entries()[0].password, "hunter2");
    }

    #[test]
    fn wrong_password_fails() {
        let bytes = sample_vault().to_bytes().unwrap();
        assert!(matches!(
            Vault::open(&bytes, "nope"),
            Err(VaultError::WrongPasswordOrCorrupt)
        ));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let mut bytes = sample_vault().to_bytes().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            Vault::open(&bytes, "master-pw"),
            Err(VaultError::WrongPasswordOrCorrupt)
        ));
    }

    #[test]
    fn tampered_header_fails() {
        let mut bytes = sample_vault().to_bytes().unwrap();
        bytes[OFF_KDF + 4] ^= 1; // change the t_cost field
        assert!(Vault::open(&bytes, "master-pw").is_err());
    }

    #[test]
    fn tampered_recovery_flag_fails() {
        // Even a header byte the password path doesn't use is authenticated.
        let mut bytes = sample_vault().to_bytes().unwrap();
        bytes[OFF_REC_FLAG] ^= 1;
        assert!(Vault::open(&bytes, "master-pw").is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(matches!(
            Vault::open(b"hello", "x"),
            Err(VaultError::InvalidFormat)
        ));
    }

    #[test]
    fn add_update_delete() {
        let mut v = sample_vault();
        let id = v.add_entry("a.com", "u", "p", "");
        v.update_entry(id, "b.com", "u2", "p2", "n").unwrap();
        assert_eq!(v.get_entry(id).unwrap().site, "b.com");
        v.delete_entry(id).unwrap();
        assert!(matches!(v.get_entry(id), Err(VaultError::EntryNotFound)));
    }

    #[test]
    fn change_password_works() {
        let mut v = sample_vault();
        // No recovery key was set, so none is returned.
        assert!(v.change_password("new-master-pw").unwrap().is_none());
        let bytes = v.to_bytes().unwrap();
        // The old password no longer opens it...
        assert!(Vault::open(&bytes, "master-pw").is_err());
        // ...the new one does, and the data is intact.
        let reopened = Vault::open(&bytes, "new-master-pw").unwrap();
        assert_eq!(reopened.entries().len(), 1);
        assert_eq!(reopened.entries()[0].password, "hunter2");
    }

    #[test]
    fn same_data_encrypts_differently_each_time() {
        let v = sample_vault();
        assert_ne!(v.to_bytes().unwrap(), v.to_bytes().unwrap());
    }

    // ----- recovery key -----

    #[test]
    fn recovery_key_unlocks_vault() {
        let mut v = sample_vault();
        assert!(!v.has_recovery_key());
        let rk = v.set_recovery_key().unwrap();
        assert!(v.has_recovery_key());
        let bytes = v.to_bytes().unwrap();

        let r = Vault::open_with_recovery(&bytes, &rk).unwrap();
        assert_eq!(r.entries()[0].password, "hunter2");

        // The master password still works too.
        assert!(Vault::open(&bytes, "master-pw").is_ok());

        // Parser ignores case, dashes and spaces.
        let sloppy = rk.to_lowercase().replace('-', " ");
        assert!(Vault::open_with_recovery(&bytes, &sloppy).is_ok());
    }

    #[test]
    fn wrong_recovery_key_fails() {
        let mut v = sample_vault();
        v.set_recovery_key().unwrap();
        let other = sample_vault().set_recovery_key().unwrap();
        let bytes = v.to_bytes().unwrap();
        assert!(matches!(
            Vault::open_with_recovery(&bytes, &other),
            Err(VaultError::WrongPasswordOrCorrupt)
        ));
        assert!(matches!(
            Vault::open_with_recovery(&bytes, "not a key"),
            Err(VaultError::InvalidRecoveryKey)
        ));
    }

    #[test]
    fn recovery_without_setup_is_refused() {
        let bytes = sample_vault().to_bytes().unwrap();
        let some_key = sample_vault().set_recovery_key().unwrap();
        assert!(matches!(
            Vault::open_with_recovery(&bytes, &some_key),
            Err(VaultError::NoRecoveryKey)
        ));
    }

    #[test]
    fn password_change_rotates_keys_and_recovery_key() {
        let mut v = sample_vault();
        let old_rk = v.set_recovery_key().unwrap();
        let old_dek = *v.dek;

        let new_rk = v.change_password("new-master-pw").unwrap().expect("new recovery key");
        assert_ne!(old_dek, *v.dek, "data key must be replaced");
        assert_ne!(old_rk, new_rk);

        let bytes = v.to_bytes().unwrap();
        assert!(Vault::open(&bytes, "master-pw").is_err());
        assert!(Vault::open(&bytes, "new-master-pw").is_ok());
        assert!(Vault::open_with_recovery(&bytes, &old_rk).is_err());
        assert!(Vault::open_with_recovery(&bytes, &new_rk).is_ok());
    }

    #[test]
    fn recover_then_set_new_password() {
        let mut v = sample_vault();
        let rk = v.set_recovery_key().unwrap();
        let bytes = v.to_bytes().unwrap();

        // "Forgot password" flow.
        let mut r = Vault::open_with_recovery(&bytes, &rk).unwrap();
        let new_rk = r.change_password("brand-new-pw").unwrap();
        assert!(new_rk.is_some());
        let bytes2 = r.to_bytes().unwrap();
        let reopened = Vault::open(&bytes2, "brand-new-pw").unwrap();
        assert_eq!(reopened.entries()[0].password, "hunter2");
    }

    // ----- quick unlock (fingerprint / face) -----

    #[test]
    fn quick_key_unlocks_without_password() {
        let v = sample_vault();
        let key = v.quick_unlock_key();
        assert_eq!(key.len(), 64);
        let bytes = v.to_bytes().unwrap();
        let opened = Vault::open_with_quick_key(&bytes, &key).unwrap();
        assert_eq!(opened.entries()[0].password, "hunter2");
    }

    #[test]
    fn quick_key_stops_working_after_password_change() {
        let mut v = sample_vault();
        let old_key = v.quick_unlock_key();
        v.change_password("new-pw").unwrap();
        let bytes = v.to_bytes().unwrap();
        assert!(Vault::open_with_quick_key(&bytes, &old_key).is_err());
        assert!(Vault::open_with_quick_key(&bytes, &v.quick_unlock_key()).is_ok());
    }

    #[test]
    fn bad_quick_keys_are_rejected() {
        let bytes = sample_vault().to_bytes().unwrap();
        let zeros = "0".repeat(64);
        let gs = "g".repeat(64);
        let cases: [&str; 5] = ["", "zz", "00", &zeros, &gs];
        for bad in cases {
            assert!(Vault::open_with_quick_key(&bytes, bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn quick_key_does_not_open_a_tampered_file() {
        let v = sample_vault();
        let key = v.quick_unlock_key();
        let mut bytes = v.to_bytes().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(Vault::open_with_quick_key(&bytes, &key).is_err());
    }

    // ----- hardening: malformed and hostile files must never panic -----
    //
    // These stand in for a fuzzer: they feed the file parser every kind of
    // damaged input we can think of. A test fails if opening a damaged file
    // panics OR succeeds.

    fn vault_with_recovery_bytes() -> (Vec<u8>, String) {
        let mut v = sample_vault();
        let key = v.set_recovery_key().unwrap();
        (v.to_bytes().unwrap(), key)
    }

    #[test]
    fn every_truncation_is_rejected() {
        let (bytes, _) = vault_with_recovery_bytes();
        for len in 0..bytes.len() {
            assert!(
                Vault::open(&bytes[..len], "master-pw").is_err(),
                "a file cut to {len} bytes was accepted"
            );
        }
    }

    #[test]
    fn extra_trailing_bytes_are_rejected() {
        let (mut bytes, _) = vault_with_recovery_bytes();
        bytes.push(0);
        assert!(Vault::open(&bytes, "master-pw").is_err());
    }

    #[test]
    fn every_single_byte_change_is_rejected() {
        let (bytes, key) = vault_with_recovery_bytes();
        for i in 0..bytes.len() {
            // Skip the memory-cost field: flipping one bit of it can
            // legitimately ask for hundreds of MiB, which only makes the
            // test slow. Absurd values are tested separately below.
            if (OFF_KDF..OFF_KDF + 4).contains(&i) {
                continue;
            }
            for bit in [0x01u8, 0x80u8] {
                let mut bad = bytes.clone();
                bad[i] ^= bit;
                assert!(
                    Vault::open(&bad, "master-pw").is_err(),
                    "changing byte {i} was not detected (password path)"
                );
                assert!(
                    Vault::open_with_recovery(&bad, &key).is_err(),
                    "changing byte {i} was not detected (recovery path)"
                );
            }
        }
    }

    #[test]
    fn absurd_kdf_settings_are_rejected_quickly() {
        let (bytes, _) = vault_with_recovery_bytes();
        for (offset, name) in [(OFF_KDF, "memory"), (OFF_KDF + 4, "passes"), (OFF_KDF + 8, "lanes")] {
            let mut bad = bytes.clone();
            bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(
                matches!(Vault::open(&bad, "master-pw"), Err(VaultError::InvalidFormat)),
                "absurd {name} setting was not rejected as invalid"
            );
        }
    }

    #[test]
    fn random_garbage_never_panics_or_opens() {
        for round in 0..300 {
            let len = (OsRng.next_u32() % 500) as usize;
            let mut buf = vec![0u8; len];
            OsRng.fill_bytes(&mut buf);
            // Half the time, start with a valid-looking header so the
            // parser gets past the first checks and goes deeper.
            if round % 2 == 0 && len > OFF_KDF + 12 {
                buf[..4].copy_from_slice(MAGIC);
                buf[4] = FORMAT_VERSION;
                buf[OFF_KDF..OFF_KDF + 4].copy_from_slice(&FAST.m_cost_kib.to_le_bytes());
                buf[OFF_KDF + 4..OFF_KDF + 8].copy_from_slice(&FAST.t_cost.to_le_bytes());
                buf[OFF_KDF + 8..OFF_KDF + 12].copy_from_slice(&FAST.p_cost.to_le_bytes());
            }
            assert!(Vault::open(&buf, "x").is_err());
            assert!(Vault::open_with_recovery(&buf, "AAAA-AAAA").is_err());
        }
    }

    #[test]
    fn calibrate_returns_sane_settings() {
        let p = calibrate_kdf(300);
        assert_eq!(p.m_cost_kib, 64 * 1024);
        assert!((3..=10).contains(&p.t_cost));
        assert_eq!(p.p_cost, 1);
    }

    // ----- categories -----

    #[test]
    fn old_entries_without_a_category_still_load() {
        let e: Entry = serde_json::from_str(
            r#"{"id":1,"site":"a.com","username":"u","password":"p","notes":""}"#,
        )
        .unwrap();
        assert_eq!(e.category, "");
        assert_eq!(sample_vault().entries()[0].category, "");
    }

    #[test]
    fn category_is_saved_and_reopened() {
        let mut v = sample_vault();
        v.set_category(0, " Social ").unwrap();
        assert_eq!(v.entries()[0].category, "social");
        let bytes = v.to_bytes().unwrap();
        let again = Vault::open(&bytes, "master-pw").unwrap();
        assert_eq!(again.entries()[0].category, "social");
        // An empty string clears it again.
        let mut again = again;
        again.set_category(0, "").unwrap();
        assert_eq!(again.entries()[0].category, "");
    }

    #[test]
    fn bad_categories_are_rejected() {
        let mut v = sample_vault();
        let long = "x".repeat(33);
        for bad in ["has space", "sym!bol", long.as_str()] {
            assert!(v.set_category(0, bad).is_err(), "accepted {bad:?}");
        }
        assert!(matches!(v.set_category(99, "bank"), Err(VaultError::EntryNotFound)));
    }
}