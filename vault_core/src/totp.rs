//! Time-based one-time passwords (TOTP, RFC 6238): the 6-digit codes that
//! authenticator apps show for two-factor login.
//!
//! The shared secret is stored inside the encrypted vault entry, like the
//! password. Codes are computed on the device from the clock; nothing is sent
//! anywhere.

use crate::{Vault, VaultError};
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Sha256, Sha512};

const DEFAULT_DIGITS: u32 = 6;
const DEFAULT_PERIOD: u32 = 30;

/// Everything needed to produce codes for one account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpConfig {
    /// The shared secret as upper-case Base32 text (no spaces or padding).
    pub secret: String,
    /// Number of digits in each code (6 to 8).
    pub digits: u32,
    /// Seconds each code stays valid (usually 30).
    pub period: u32,
    /// "SHA1", "SHA256" or "SHA512" (almost every service uses SHA1).
    pub algorithm: String,
}

/// A code and how long it stays valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TotpCode {
    pub code: String,
    pub seconds_remaining: u32,
    pub period: u32,
}

fn totp_error(msg: &str) -> VaultError {
    VaultError::Totp(msg.to_string())
}

/// Base32 decoder that tolerates the sloppy padding some services produce.
fn decode_secret(secret: &str) -> Result<Vec<u8>, VaultError> {
    let mut spec = BASE32_NOPAD.specification();
    spec.check_trailing_bits = false;
    let encoding = spec
        .encoding()
        .map_err(|_| totp_error("internal error: bad Base32 settings"))?;
    encoding.decode(secret.as_bytes()).map_err(|_| {
        totp_error(
            "that setup key is not valid (it should only use the letters A to Z \
             and the digits 2 to 7)",
        )
    })
}

fn clean_secret(raw: &str) -> String {
    raw.replace("%3D", "")
        .replace("%3d", "")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '=')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Accepts what people actually paste: a bare setup key (spaces and lower case
/// are fine) or a full `otpauth://totp/...` link.
pub fn parse_totp_input(input: &str) -> Result<TotpConfig, VaultError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(totp_error("enter a setup key"));
    }

    let mut secret = text.to_string();
    let mut digits = DEFAULT_DIGITS;
    let mut period = DEFAULT_PERIOD;
    let mut algorithm = "SHA1".to_string();

    const SCHEME: &str = "otpauth://";
    if text.to_ascii_lowercase().starts_with(SCHEME) {
        let rest = &text[SCHEME.len()..];
        let (kind, after) = rest
            .split_once('/')
            .ok_or_else(|| totp_error("that link is not a valid 2FA link"))?;
        if !kind.eq_ignore_ascii_case("totp") {
            return Err(totp_error("only time-based (TOTP) codes are supported"));
        }
        let query = after
            .split_once('?')
            .map(|(_, q)| q)
            .ok_or_else(|| totp_error("that link has no secret in it"))?;

        let mut found = None;
        for pair in query.split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            match key.to_ascii_lowercase().as_str() {
                "secret" => found = Some(value.to_string()),
                "digits" => {
                    digits = value.parse().map_err(|_| totp_error("invalid number of digits"))?
                }
                "period" => {
                    period = value.parse().map_err(|_| totp_error("invalid period"))?
                }
                "algorithm" => algorithm = value.to_ascii_uppercase(),
                _ => {}
            }
        }
        secret = found.ok_or_else(|| totp_error("that link has no secret in it"))?;
    }

    let secret = clean_secret(&secret);
    let key = decode_secret(&secret)?;
    if key.len() < 8 {
        return Err(totp_error("that setup key is too short"));
    }
    if key.len() > 128 {
        return Err(totp_error("that setup key is too long"));
    }
    if !(6..=8).contains(&digits) {
        return Err(totp_error("codes must have 6, 7 or 8 digits"));
    }
    if !(10..=300).contains(&period) {
        return Err(totp_error("unsupported code period"));
    }
    if !["SHA1", "SHA256", "SHA512"].contains(&algorithm.as_str()) {
        return Err(totp_error("unsupported algorithm (use SHA1, SHA256 or SHA512)"));
    }
    Ok(TotpConfig { secret, digits, period, algorithm })
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha512(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha512>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

impl TotpConfig {
    /// The text to show when editing: just the secret when every setting is
    /// the usual default, otherwise a full link that keeps the settings.
    /// `parse_totp_input` accepts both, so editing never changes anything.
    pub fn to_input(&self) -> String {
        if self.digits == DEFAULT_DIGITS
            && self.period == DEFAULT_PERIOD
            && self.algorithm == "SHA1"
        {
            self.secret.clone()
        } else {
            format!(
                "otpauth://totp/Vault?secret={}&algorithm={}&digits={}&period={}",
                self.secret, self.algorithm, self.digits, self.period
            )
        }
    }

    /// The code for the given time (seconds since 1970-01-01 UTC).
    pub fn code_at(&self, unix_time: u64) -> Result<TotpCode, VaultError> {
        let period = self.period.max(1);
        let key = decode_secret(&self.secret)?;
        let counter = unix_time / u64::from(period);
        let message = counter.to_be_bytes();

        let digest = match self.algorithm.as_str() {
            "SHA256" => hmac_sha256(&key, &message),
            "SHA512" => hmac_sha512(&key, &message),
            _ => hmac_sha1(&key, &message),
        };

        // "Dynamic truncation" from RFC 4226.
        let offset = (digest[digest.len() - 1] & 0x0f) as usize;
        let bits = u32::from_be_bytes([
            digest[offset],
            digest[offset + 1],
            digest[offset + 2],
            digest[offset + 3],
        ]) & 0x7fff_ffff;
        let digits = self.digits.clamp(6, 8);
        let number = bits % 10u32.pow(digits);

        Ok(TotpCode {
            code: format!("{:0width$}", number, width = digits as usize),
            seconds_remaining: period - (unix_time % u64::from(period)) as u32,
            period,
        })
    }
}

impl Vault {
    /// Set, replace, or (with an empty string) remove an entry's 2FA secret.
    /// Call `save` afterwards.
    pub fn set_totp(&mut self, id: u64, input: &str) -> Result<(), VaultError> {
        let config = if input.trim().is_empty() {
            None
        } else {
            Some(parse_totp_input(input)?)
        };
        let entry = self
            .data
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(VaultError::EntryNotFound)?;
        entry.totp = config;
        entry.updated = crate::now_ms();
        Ok(())
    }

    /// The current code for an entry, or `None` if it has no 2FA secret.
    pub fn totp_code(&self, id: u64, unix_time: u64) -> Result<Option<TotpCode>, VaultError> {
        let entry = self.get_entry(id)?;
        match &entry.totp {
            None => Ok(None),
            Some(config) => config.code_at(unix_time).map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KdfParams;

    const FAST: KdfParams = KdfParams { m_cost_kib: 8, t_cost: 1, p_cost: 1 };

    fn config(secret_bytes: &[u8], digits: u32, algorithm: &str) -> TotpConfig {
        TotpConfig {
            secret: BASE32_NOPAD.encode(secret_bytes),
            digits,
            period: 30,
            algorithm: algorithm.to_string(),
        }
    }

    // Test vectors from RFC 6238, Appendix B (8-digit codes).
    #[test]
    fn rfc6238_sha1_vectors() {
        let c = config(b"12345678901234567890", 8, "SHA1");
        let cases = [
            (59u64, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ];
        for (time, expected) in cases {
            assert_eq!(c.code_at(time).unwrap().code, expected, "time {time}");
        }
    }

    #[test]
    fn rfc6238_sha256_and_sha512_vectors() {
        let c256 = config(b"12345678901234567890123456789012", 8, "SHA256");
        assert_eq!(c256.code_at(59).unwrap().code, "46119246");
        let c512 = config(
            b"1234567890123456789012345678901234567890123456789012345678901234",
            8,
            "SHA512",
        );
        assert_eq!(c512.code_at(59).unwrap().code, "90693936");
    }

    #[test]
    fn six_digit_codes_keep_leading_zeros() {
        // 8-digit code 07081804 -> last 6 digits are 081804.
        let c = config(b"12345678901234567890", 6, "SHA1");
        assert_eq!(c.code_at(1_111_111_109).unwrap().code, "081804");
    }

    #[test]
    fn countdown_is_correct() {
        let c = config(b"12345678901234567890", 6, "SHA1");
        assert_eq!(c.code_at(60).unwrap().seconds_remaining, 30);
        assert_eq!(c.code_at(61).unwrap().seconds_remaining, 29);
        assert_eq!(c.code_at(89).unwrap().seconds_remaining, 1);
        assert_eq!(c.code_at(60).unwrap().period, 30);
    }

    #[test]
    fn plain_keys_are_cleaned_up() {
        let c = parse_totp_input(" jbsw y3dp-ehpk 3pxp ").unwrap();
        assert_eq!(c.secret, "JBSWY3DPEHPK3PXP");
        assert_eq!((c.digits, c.period, c.algorithm.as_str()), (6, 30, "SHA1"));
    }

    #[test]
    fn otpauth_links_are_understood() {
        let link = "otpauth://totp/Example:me@mail.com?secret=jbswy3dpehpk3pxp\
                    &issuer=Example&algorithm=sha256&digits=8&period=60";
        let c = parse_totp_input(link).unwrap();
        assert_eq!(c.secret, "JBSWY3DPEHPK3PXP");
        assert_eq!((c.digits, c.period, c.algorithm.as_str()), (8, 60, "SHA256"));
    }

    #[test]
    fn editing_text_round_trips() {
        let plain = parse_totp_input("JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(plain.to_input(), "JBSWY3DPEHPK3PXP");
        assert_eq!(parse_totp_input(&plain.to_input()).unwrap(), plain);

        let custom = TotpConfig {
            secret: "JBSWY3DPEHPK3PXP".into(),
            digits: 8,
            period: 60,
            algorithm: "SHA512".into(),
        };
        assert_eq!(parse_totp_input(&custom.to_input()).unwrap(), custom);
    }

    #[test]
    fn bad_input_is_rejected() {
        for bad in [
            "",
            "   ",
            "not base32!!",
            "AAAA",                                                  // too short
            "otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=1",    // not time-based
            "otpauth://totp/x",                                      // no secret
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=9",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=0",
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=MD5",
        ] {
            assert!(parse_totp_input(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn entries_can_hold_a_secret_and_survive_save_and_reopen() {
        let mut v = Vault::create("master-pw", FAST).unwrap();
        let id = v.add_entry("example.com", "me", "pw", "");
        assert!(v.totp_code(id, 59).unwrap().is_none());

        v.set_totp(id, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap();
        let code = v.totp_code(id, 59).unwrap().unwrap();
        assert_eq!(code.code.len(), 6);

        let bytes = v.to_bytes().unwrap();
        let again = Vault::open(&bytes, "master-pw").unwrap();
        assert_eq!(again.totp_code(id, 59).unwrap().unwrap(), code);

        // An empty string removes it again.
        let mut again = again;
        again.set_totp(id, "").unwrap();
        assert!(again.totp_code(id, 59).unwrap().is_none());
    }

    #[test]
    fn setting_a_bad_secret_leaves_the_entry_unchanged() {
        let mut v = Vault::create("master-pw", FAST).unwrap();
        let id = v.add_entry("a.com", "me", "pw", "");
        v.set_totp(id, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap();
        assert!(v.set_totp(id, "nonsense!!").is_err());
        assert!(v.totp_code(id, 59).unwrap().is_some());
        assert!(matches!(v.set_totp(99, "JBSWY3DPEHPK3PXP"), Err(VaultError::EntryNotFound)));
    }
}