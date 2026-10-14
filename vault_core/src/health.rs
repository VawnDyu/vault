//! Password health: finds reused and weak passwords inside an unlocked vault.
//!
//! Everything happens in memory on the device. Nothing is sent anywhere, and
//! no password is ever compared against an online list.

use crate::Vault;
use std::collections::{HashMap, HashSet};

/// Why a password was judged weak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakReason {
    TooShort,
    CommonPassword,
    OnlyDigits,
    Repetitive,
    Sequential,
    LowVariety,
}

impl WeakReason {
    /// A short, user-facing explanation.
    pub fn describe(self) -> &'static str {
        match self {
            WeakReason::TooShort => "Too short (use at least 8 characters)",
            WeakReason::CommonPassword => "A very common password",
            WeakReason::OnlyDigits => "Only digits",
            WeakReason::Repetitive => "Repeats the same few characters",
            WeakReason::Sequential => "An easy-to-guess sequence",
            WeakReason::LowVariety => "Short and uses only one kind of character",
        }
    }
}

/// A small offline list of passwords that are guessed first by attackers.
const COMMON: &[&str] = &[
    "password", "password1", "password12", "password123", "passw0rd", "p@ssw0rd",
    "12345678", "123456789", "1234567890", "11111111", "00000000", "87654321",
    "qwertyui", "qwertyuiop", "qwerty123", "qwerty1234", "1q2w3e4r", "1qaz2wsx",
    "zaq12wsx", "qazwsxedc", "iloveyou", "iloveyou1", "admin123", "admin1234",
    "welcome1", "welcome123", "letmein1", "letmein123", "monkey123", "dragon123",
    "football1", "baseball1", "sunshine1", "princess1", "superman1", "trustno1",
    "whatever1", "changeme", "default1", "login123", "secret123", "master123",
    "hello123", "freedom1", "abc12345", "abcd1234", "test1234", "asdfghjk",
];

/// Keyboard and alphabet runs. A password that is a piece of one of these
/// (forwards or backwards) is easy to guess.
const SEQUENCES: &[&str] = &[
    "abcdefghijklmnopqrstuvwxyz",
    "0123456789",
    "qwertyuiopasdfghjklzxcvbnm",
    "1qaz2wsx3edc4rfv5tgb",
];

/// Returns why a password is weak, or `None` if it looks acceptable.
/// This is a quick sanity check, not a full strength estimate.
pub fn weakness(password: &str) -> Option<WeakReason> {
    let len = password.chars().count();
    if len < 8 {
        return Some(WeakReason::TooShort);
    }
    let lower = password.to_lowercase();
    if COMMON.contains(&lower.as_str()) || lower.starts_with("password") {
        return Some(WeakReason::CommonPassword);
    }
    if password.chars().all(|c| c.is_ascii_digit()) {
        return Some(WeakReason::OnlyDigits);
    }
    let mut distinct: Vec<char> = lower.chars().collect();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() <= 2 {
        return Some(WeakReason::Repetitive);
    }
    for seq in SEQUENCES {
        let backwards: String = seq.chars().rev().collect();
        if seq.contains(lower.as_str()) || backwards.contains(lower.as_str()) {
            return Some(WeakReason::Sequential);
        }
    }
    let has_lower = password.chars().any(|c| c.is_lowercase());
    let has_upper = password.chars().any(|c| c.is_uppercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_other = password
        .chars()
        .any(|c| !c.is_alphanumeric());
    let kinds = [has_lower, has_upper, has_digit, has_other]
        .iter()
        .filter(|k| **k)
        .count();
    if len < 12 && kinds <= 1 {
        return Some(WeakReason::LowVariety);
    }
    None
}

/// The result of checking a whole vault.
#[derive(Debug, Default)]
pub struct HealthReport {
    /// Number of entries in the vault.
    pub total: usize,
    /// Groups of entry ids that share exactly the same password.
    pub reused_groups: Vec<Vec<u64>>,
    /// Entries with a weak password, and why.
    pub weak: Vec<(u64, WeakReason)>,
}

impl HealthReport {
    /// Ids of every entry that has at least one problem.
    pub fn unhealthy_ids(&self) -> HashSet<u64> {
        let mut ids: HashSet<u64> = self.weak.iter().map(|(id, _)| *id).collect();
        for group in &self.reused_groups {
            ids.extend(group.iter().copied());
        }
        ids
    }
}

impl Vault {
    /// Check every entry for reused and weak passwords.
    pub fn health_report(&self) -> HealthReport {
        let entries = self.entries();
        let mut by_password: HashMap<&str, Vec<u64>> = HashMap::new();
        let mut weak = Vec::new();

        for e in entries {
            if e.password.is_empty() {
                continue;
            }
            by_password.entry(e.password.as_str()).or_default().push(e.id);
            if let Some(reason) = weakness(&e.password) {
                weak.push((e.id, reason));
            }
        }

        let mut reused_groups: Vec<Vec<u64>> = by_password
            .into_values()
            .filter(|group| group.len() > 1)
            .collect();
        for group in reused_groups.iter_mut() {
            group.sort_unstable();
        }
        reused_groups.sort(); // stable, predictable order

        HealthReport { total: entries.len(), reused_groups, weak }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{self, GeneratorOptions};
    use crate::KdfParams;

    const FAST: KdfParams = KdfParams { m_cost_kib: 8, t_cost: 1, p_cost: 1 };

    fn vault_with(passwords: &[&str]) -> Vault {
        let mut v = Vault::create("master-pw", FAST).unwrap();
        for (i, pw) in passwords.iter().enumerate() {
            v.add_entry(&format!("site{i}.com"), "me", pw, "");
        }
        v
    }

    #[test]
    fn weak_passwords_are_flagged_with_a_reason() {
        let cases: &[(&str, Option<WeakReason>)] = &[
            ("short1!", Some(WeakReason::TooShort)),
            ("", Some(WeakReason::TooShort)),
            ("password123", Some(WeakReason::CommonPassword)),
            ("PASSWORD!!!!", Some(WeakReason::CommonPassword)),
            ("12345678", Some(WeakReason::CommonPassword)),
            ("987654321", Some(WeakReason::OnlyDigits)),
            ("aaaaaaaaaa", Some(WeakReason::Repetitive)),
            ("abababababab", Some(WeakReason::Repetitive)),
            ("abcdefgh", Some(WeakReason::Sequential)),
            ("hgfedcba", Some(WeakReason::Sequential)),
            ("asdfghjkl", Some(WeakReason::Sequential)),
            ("sunnyday", Some(WeakReason::LowVariety)),
        ];
        for (pw, expected) in cases {
            assert_eq!(weakness(pw), *expected, "password {pw:?}");
        }
    }

    #[test]
    fn reasonable_passwords_are_not_flagged() {
        for pw in [
            "Tr0ub4dor&3",
            "correct horse battery staple",
            "Summer2024!x",
            "a-long-lowercase-passphrase",
            "xK9#mP2$vL8@",
        ] {
            assert_eq!(weakness(pw), None, "password {pw:?}");
        }
    }

    #[test]
    fn generated_passwords_are_never_weak() {
        let opts = GeneratorOptions::default();
        for _ in 0..200 {
            let pw = generator::generate(&opts).unwrap();
            assert_eq!(weakness(&pw), None, "generated {pw:?}");
        }
    }

    #[test]
    fn reused_passwords_are_grouped() {
        let v = vault_with(&["xK9#mP2$vL8@", "unique-Pass-1!", "xK9#mP2$vL8@", "xK9#mP2$vL8@"]);
        let r = v.health_report();
        assert_eq!(r.total, 4);
        assert_eq!(r.reused_groups, vec![vec![0, 2, 3]]);
        assert!(r.weak.is_empty());
    }

    #[test]
    fn two_separate_reuse_groups() {
        let v = vault_with(&["xK9#mP2$vL8@", "yR7!qW3&zN5#", "xK9#mP2$vL8@", "yR7!qW3&zN5#"]);
        let r = v.health_report();
        assert_eq!(r.reused_groups, vec![vec![0, 2], vec![1, 3]]);
    }

    #[test]
    fn an_entry_can_be_both_weak_and_reused() {
        let v = vault_with(&["password123", "password123", "xK9#mP2$vL8@"]);
        let r = v.health_report();
        assert_eq!(r.reused_groups, vec![vec![0, 1]]);
        assert_eq!(r.weak.len(), 2);
        // entries 0 and 1 are unhealthy once each; entry 2 is fine
        let unhealthy = r.unhealthy_ids();
        assert_eq!(unhealthy.len(), 2);
        assert!(unhealthy.contains(&0) && unhealthy.contains(&1));
    }

    #[test]
    fn empty_vault_has_nothing_to_report() {
        let v = vault_with(&[]);
        let r = v.health_report();
        assert_eq!(r.total, 0);
        assert!(r.reused_groups.is_empty() && r.weak.is_empty());
    }

    #[test]
    fn empty_passwords_are_ignored_for_reuse() {
        let mut v = Vault::create("master-pw", FAST).unwrap();
        v.add_entry("a.com", "me", "", "");
        v.add_entry("b.com", "me", "", "");
        let r = v.health_report();
        assert!(r.reused_groups.is_empty());
        assert!(r.weak.is_empty());
    }
}