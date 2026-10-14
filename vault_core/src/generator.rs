//! Random password generator.
//!
//! Uses the operating system's secure random source (OsRng). Never use a
//! normal/seeded random generator for passwords.

use rand::{rngs::OsRng, seq::SliceRandom, Rng};
use std::fmt;
use zeroize::Zeroizing;

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*()-_=+?";
/// Characters that are easy to confuse when reading: I l 1 O 0
const AMBIGUOUS: &str = "Il1O0";

pub const MAX_LENGTH: usize = 1024;

#[derive(Debug, Clone)]
pub struct GeneratorOptions {
    pub length: usize,
    pub lowercase: bool,
    pub uppercase: bool,
    pub digits: bool,
    pub symbols: bool,
    pub exclude_ambiguous: bool,
}

impl Default for GeneratorOptions {
    fn default() -> Self {
        GeneratorOptions {
            length: 20,
            lowercase: true,
            uppercase: true,
            digits: true,
            symbols: true,
            exclude_ambiguous: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum GeneratorError {
    NoCharacterSets,
    /// Length is smaller than the number of selected character types.
    LengthTooShort,
    LengthTooLong,
}

impl fmt::Display for GeneratorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeneratorError::NoCharacterSets => write!(f, "select at least one character type"),
            GeneratorError::LengthTooShort => {
                write!(f, "length is too short for the selected character types")
            }
            GeneratorError::LengthTooLong => write!(f, "length must be at most {MAX_LENGTH}"),
        }
    }
}

impl std::error::Error for GeneratorError {}

/// One list of characters per selected character type.
fn pools(opts: &GeneratorOptions) -> Vec<Vec<char>> {
    let mut result = Vec::new();
    let sets = [
        (opts.lowercase, LOWER),
        (opts.uppercase, UPPER),
        (opts.digits, DIGITS),
        (opts.symbols, SYMBOLS),
    ];
    for (enabled, set) in sets {
        if enabled {
            let chars: Vec<char> = set
                .chars()
                .filter(|c| !(opts.exclude_ambiguous && AMBIGUOUS.contains(*c)))
                .collect();
            result.push(chars);
        }
    }
    result
}

/// Generate a password. Guarantees at least one character from every
/// selected type, then shuffles so the positions are not predictable.
pub fn generate(opts: &GeneratorOptions) -> Result<String, GeneratorError> {
    let pools = pools(opts);
    if pools.is_empty() {
        return Err(GeneratorError::NoCharacterSets);
    }
    if opts.length < pools.len() {
        return Err(GeneratorError::LengthTooShort);
    }
    if opts.length > MAX_LENGTH {
        return Err(GeneratorError::LengthTooLong);
    }

    let all: Vec<char> = pools.iter().flatten().copied().collect();
    let mut out: Zeroizing<Vec<char>> = Zeroizing::new(Vec::with_capacity(opts.length));

    // One guaranteed character from each selected type.
    for pool in &pools {
        out.push(*pool.choose(&mut OsRng).expect("pool is never empty"));
    }
    // Fill the rest from all selected types combined.
    // gen_range is unbiased (no "modulo bias").
    while out.len() < opts.length {
        out.push(all[OsRng.gen_range(0..all.len())]);
    }
    out.shuffle(&mut OsRng);

    Ok(out.iter().collect())
}

/// Approximate strength in bits (length x log2 of the character pool size).
/// Around 80+ bits is strong; 100+ is excellent.
pub fn entropy_bits(opts: &GeneratorOptions) -> f64 {
    let pool_size: usize = pools(opts).iter().map(|p| p.len()).sum();
    if pool_size == 0 {
        0.0
    } else {
        opts.length as f64 * (pool_size as f64).log2()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_requested_length() {
        for len in [4, 8, 20, 64, 200] {
            let opts = GeneratorOptions { length: len, ..Default::default() };
            assert_eq!(generate(&opts).unwrap().chars().count(), len);
        }
    }

    #[test]
    fn contains_every_selected_type() {
        // Short length makes it most likely to fail if the guarantee is broken.
        let opts = GeneratorOptions { length: 4, ..Default::default() };
        for _ in 0..200 {
            let p = generate(&opts).unwrap();
            assert!(p.chars().any(|c| LOWER.contains(c)));
            assert!(p.chars().any(|c| UPPER.contains(c)));
            assert!(p.chars().any(|c| DIGITS.contains(c)));
            assert!(p.chars().any(|c| SYMBOLS.contains(c)));
        }
    }

    #[test]
    fn only_uses_selected_types() {
        let opts = GeneratorOptions {
            length: 50,
            uppercase: false,
            symbols: false,
            ..Default::default()
        };
        for _ in 0..100 {
            let p = generate(&opts).unwrap();
            assert!(p.chars().all(|c| LOWER.contains(c) || DIGITS.contains(c)));
        }
    }

    #[test]
    fn excludes_ambiguous_characters() {
        let opts = GeneratorOptions {
            length: 100,
            exclude_ambiguous: true,
            ..Default::default()
        };
        for _ in 0..100 {
            let p = generate(&opts).unwrap();
            assert!(!p.chars().any(|c| AMBIGUOUS.contains(c)));
        }
    }

    #[test]
    fn rejects_bad_options() {
        let none = GeneratorOptions {
            lowercase: false,
            uppercase: false,
            digits: false,
            symbols: false,
            ..Default::default()
        };
        assert_eq!(generate(&none), Err(GeneratorError::NoCharacterSets));

        let short = GeneratorOptions { length: 3, ..Default::default() };
        assert_eq!(generate(&short), Err(GeneratorError::LengthTooShort));

        let long = GeneratorOptions { length: MAX_LENGTH + 1, ..Default::default() };
        assert_eq!(generate(&long), Err(GeneratorError::LengthTooLong));
    }

    #[test]
    fn outputs_differ() {
        let opts = GeneratorOptions::default();
        assert_ne!(generate(&opts).unwrap(), generate(&opts).unwrap());
    }

    #[test]
    fn entropy_is_sensible() {
        let opts = GeneratorOptions::default();
        assert!(entropy_bits(&opts) > 100.0); // 20 chars, ~77-char pool
    }
}