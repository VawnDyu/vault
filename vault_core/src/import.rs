//! Import passwords from the CSV files that browsers and other password
//! managers export (Chrome, Edge, Firefox, Bitwarden and similar).
//!
//! The file is untrusted input, so parsing is defensive: size limits, no
//! panics, and anything unusable is skipped and counted instead of aborting.
//!
//! Note: a CSV export holds passwords as plain text. This module only reads
//! it from memory; deleting the file afterwards is the user's job.

use crate::{Vault, VaultError};
use std::collections::HashSet;

/// Largest CSV file we will look at (10 MiB).
const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;
/// Most entries one import may add.
const MAX_IMPORT_ENTRIES: usize = 20_000;

/// What happened during an import.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// Entries added to the vault.
    pub added: u32,
    /// Rows skipped because the vault (or the file) already had the same
    /// site + username + password.
    pub duplicates: u32,
    /// Rows we could not use (no password, no site, secure notes, bad rows).
    pub skipped: u32,
}

/// One usable row from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedEntry {
    pub site: String,
    pub username: String,
    pub password: String,
    pub notes: String,
    /// 2FA secret or otpauth link, if the export had one (Bitwarden does).
    pub totp: String,
}

fn import_error(msg: &str) -> VaultError {
    VaultError::Import(msg.to_string())
}

/// Lowercase, trim, and drop a leading byte-order mark (Chrome adds one).
fn normalize_header(s: &str) -> String {
    s.trim_start_matches('\u{feff}').trim().to_lowercase()
}

/// Index of the first header that matches one of `names` (in priority order).
fn find_column(headers: &[String], names: &[&str]) -> Option<usize> {
    names
        .iter()
        .find_map(|n| headers.iter().position(|h| h == n))
}

/// Parse a CSV export into entries. Returns the usable entries and how many
/// rows were skipped.
pub fn parse_csv(bytes: &[u8]) -> Result<(Vec<ImportedEntry>, u32), VaultError> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(import_error("that file is too large to import (limit 10 MB)"));
    }
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(bytes);

    let headers: Vec<String> = reader
        .headers()
        .map_err(|_| import_error("could not read the CSV header row"))?
        .iter()
        .map(normalize_header)
        .collect();

    let col_name = find_column(&headers, &["name", "title", "site", "website", "web site"]);
    let col_url = find_column(&headers, &["url", "login_uri", "uri", "web address", "origin"]);
    let col_user = find_column(
        &headers,
        &["username", "login_username", "user name", "user", "login", "email"],
    );
    let col_pass = find_column(&headers, &["password", "login_password", "pass"]);
    let col_note = find_column(&headers, &["note", "notes", "extra", "comments"]);
    let col_totp = find_column(&headers, &["login_totp", "totp", "otp"]);
    let col_type = find_column(&headers, &["type"]); // Bitwarden: login / note / card / identity

    if col_pass.is_none() || (col_name.is_none() && col_url.is_none()) {
        return Err(import_error(
            "this doesn't look like a password export; expected columns such as \
             name, url, username and password",
        ));
    }

    let mut entries = Vec::new();
    let mut skipped: u32 = 0;

    for record in reader.records() {
        let record = match record {
            Ok(r) => r,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let text = |col: Option<usize>| -> String {
            col.and_then(|i| record.get(i)).unwrap_or("").trim().to_string()
        };

        // Bitwarden exports notes, cards and identities too. Keep logins only.
        if let Some(i) = col_type {
            let kind = record.get(i).unwrap_or("").trim().to_lowercase();
            if !kind.is_empty() && kind != "login" {
                skipped += 1;
                continue;
            }
        }

        let name = text(col_name);
        let url = text(col_url);
        // Passwords are NOT trimmed: spaces can be part of a password.
        let password = col_pass.and_then(|i| record.get(i)).unwrap_or("").to_string();

        let site = if name.is_empty() { url.clone() } else { name.clone() };
        if site.is_empty() || password.is_empty() {
            skipped += 1;
            continue;
        }

        let mut notes = text(col_note);
        // Keep the full address when the title is something shorter.
        if !name.is_empty() && !url.is_empty() && !url.eq_ignore_ascii_case(&name) {
            notes = if notes.is_empty() {
                format!("URL: {url}")
            } else {
                format!("URL: {url}\n{notes}")
            };
        }

        entries.push(ImportedEntry {
            site,
            username: text(col_user),
            password,
            notes,
            totp: text(col_totp),
        });
        if entries.len() > MAX_IMPORT_ENTRIES {
            return Err(import_error("that file has too many entries to import at once"));
        }
    }
    Ok((entries, skipped))
}

/// Best-guess category for an imported entry, from well-known site names.
/// Returns "" when nothing matches (the entry stays uncategorized). This is
/// only a convenience: every guess can be changed in the app.
pub fn guess_category(site: &str, extra: &str) -> &'static str {
    const RULES: &[(&str, &[&str])] = &[
        ("social", &[
            "facebook", "instagram", "twitter", "tiktok", "snapchat", "linkedin",
            "reddit", "pinterest", "whatsapp", "telegram", "discord", "tumblr",
            "messenger", "wechat", "viber", "threads.net", "mastodon",
        ]),
        ("bank", &[
            "bank", "paypal", "wise.com", "revolut", "mastercard", "stripe",
            "coinbase", "binance", "fidelity", "schwab", "vanguard", "americanexpress",
            "wellsfargo", "chase.com", "citi.com", "capitalone",
        ]),
        ("email", &["gmail", "outlook", "yahoo", "protonmail", "proton.me", "icloud", "zoho"]),
        ("shopping", &[
            "amazon", "ebay", "aliexpress", "shopee", "lazada", "etsy", "walmart",
            "shopify", "temu", "shein", "ikea", "zalando",
        ]),
        ("entertainment", &[
            "netflix", "spotify", "youtube", "hulu", "disneyplus", "hbomax", "twitch",
            "primevideo", "crunchyroll", "deezer", "soundcloud", "imdb",
        ]),
        ("gaming", &[
            "steampowered", "steamcommunity", "epicgames", "playstation", "xbox",
            "nintendo", "riotgames", "blizzard", "ubisoft", "roblox", "minecraft",
        ]),
        ("work", &[
            "slack", "github", "gitlab", "atlassian", "jira", "notion.so", "zoom.us",
            "microsoft", "salesforce", "asana", "trello", "dropbox", "figma",
        ]),
    ];
    let haystack = format!("{} {}", site.to_lowercase(), extra.to_lowercase());
    for (category, words) in RULES {
        if words.iter().any(|w| haystack.contains(*w)) {
            return *category;
        }
    }
    ""
}

/// Identity of an entry for duplicate detection.
fn dedupe_key(site: &str, username: &str, password: &str) -> (String, String, String) {
    (site.trim().to_lowercase(), username.trim().to_lowercase(), password.to_string())
}

impl Vault {
    /// Add the entries from a CSV export. Entries that are already in the
    /// vault (same site, username and password) are skipped, and so are
    /// repeats inside the file. Call `save` afterwards.
    pub fn import_csv(&mut self, bytes: &[u8]) -> Result<ImportReport, VaultError> {
        let (parsed, skipped) = parse_csv(bytes)?;

        let mut seen: HashSet<(String, String, String)> = self
            .entries()
            .iter()
            .map(|e| dedupe_key(&e.site, &e.username, &e.password))
            .collect();

        let mut report = ImportReport { skipped, ..Default::default() };
        for p in parsed {
            if !seen.insert(dedupe_key(&p.site, &p.username, &p.password)) {
                report.duplicates += 1;
                continue;
            }
            let id = self.add_entry(&p.site, &p.username, &p.password, &p.notes);
            let guess = guess_category(&p.site, &p.notes);
            if !guess.is_empty() {
                let _ = self.set_category(id, guess);
            }
            // A secret we can't read is simply skipped; the login is still added.
            if !p.totp.is_empty() {
                let _ = self.set_totp(id, &p.totp);
            }
            report.added += 1;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KdfParams;

    const FAST: KdfParams = KdfParams { m_cost_kib: 8, t_cost: 1, p_cost: 1 };

    fn empty_vault() -> Vault {
        Vault::create("master-pw", FAST).unwrap()
    }

    #[test]
    fn chrome_format() {
        let csv = "name,url,username,password,note\n\
                   example.com,https://example.com/login,me@mail.com,hunter2,my note\n\
                   github.com,https://github.com/login,octocat,s3cret,\n";
        let mut v = empty_vault();
        let r = v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(r, ImportReport { added: 2, duplicates: 0, skipped: 0 });
        let e = &v.entries()[0];
        assert_eq!(e.site, "example.com");
        assert_eq!(e.username, "me@mail.com");
        assert_eq!(e.password, "hunter2");
        assert!(e.notes.contains("https://example.com/login"));
        assert!(e.notes.contains("my note"));
    }

    #[test]
    fn firefox_format_uses_url_as_site() {
        let csv = "url,username,password,httpRealm,formActionOrigin,guid\n\
                   https://example.org,bob,pw123,,,{abc}\n";
        let mut v = empty_vault();
        v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(v.entries()[0].site, "https://example.org");
        assert_eq!(v.entries()[0].username, "bob");
    }

    #[test]
    fn bitwarden_format_keeps_logins_only() {
        let csv = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
                   ,,login,Netflix,watch,,0,https://netflix.com,me,pw-one,\n\
                   ,,note,Wifi code,secret note,,0,,,,\n\
                   ,,card,My card,,,0,,,,\n";
        let mut v = empty_vault();
        let r = v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(r.added, 1);
        assert_eq!(r.skipped, 2);
        assert_eq!(v.entries()[0].site, "Netflix");
        assert_eq!(v.entries()[0].password, "pw-one");
    }

    #[test]
    fn handles_byte_order_mark_and_quoting() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            b"name,url,username,password,note\n\
              \"Shop, Inc\",https://shop.example,me,\"pa\"\"ss,word\",\"line1\nline2\"\n",
        );
        let mut v = empty_vault();
        let r = v.import_csv(&bytes).unwrap();
        assert_eq!(r.added, 1);
        let e = &v.entries()[0];
        assert_eq!(e.site, "Shop, Inc");
        assert_eq!(e.password, "pa\"ss,word");
        assert!(e.notes.contains("line1\nline2"));
    }

    #[test]
    fn passwords_keep_their_spaces() {
        let csv = "name,url,username,password\nsite,,u,\" spaced pw \"\n";
        let mut v = empty_vault();
        v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(v.entries()[0].password, " spaced pw ");
    }

    #[test]
    fn rows_without_site_or_password_are_skipped() {
        let csv = "name,url,username,password\n\
                   ok.com,,me,pw\n\
                   nopass.com,,me,\n\
                   ,,me,pw\n";
        let mut v = empty_vault();
        let r = v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(r.added, 1);
        assert_eq!(r.skipped, 2);
    }

    #[test]
    fn duplicates_are_skipped() {
        let csv = "name,url,username,password\n\
                   a.com,,me,pw\n\
                   A.com,,ME,pw\n\
                   b.com,,me,pw\n";
        let mut v = empty_vault();
        v.add_entry("b.com", "me", "pw", ""); // already in the vault
        let r = v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(r.added, 1); // only the first a.com
        assert_eq!(r.duplicates, 2); // A.com (same as a.com) and b.com (already there)
        assert_eq!(v.entries().len(), 2);
    }

    #[test]
    fn importing_twice_adds_nothing_new() {
        let csv = "name,url,username,password\na.com,,me,pw\nb.com,,me,pw2\n";
        let mut v = empty_vault();
        v.import_csv(csv.as_bytes()).unwrap();
        let r = v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(r.added, 0);
        assert_eq!(r.duplicates, 2);
    }

    #[test]
    fn non_password_files_are_rejected() {
        let mut v = empty_vault();
        assert!(v.import_csv(b"fruit,color\napple,red\n").is_err());
        assert!(v.import_csv(b"").is_err());
        assert!(v.import_csv(&[0xFF, 0xFE, 0x00, 0x01]).is_err());
    }

    #[test]
    fn oversized_files_are_rejected() {
        let mut v = empty_vault();
        let big = vec![b'a'; MAX_IMPORT_BYTES + 1];
        assert!(v.import_csv(&big).is_err());
    }

    #[test]
    fn imported_entries_survive_save_and_reopen() {
        let csv = "name,url,username,password\na.com,,me,pw\n";
        let mut v = empty_vault();
        v.import_csv(csv.as_bytes()).unwrap();
        let bytes = v.to_bytes().unwrap();
        let again = Vault::open(&bytes, "master-pw").unwrap();
        assert_eq!(again.entries()[0].site, "a.com");
    }

    #[test]
    fn random_garbage_never_panics() {
        use rand::{rngs::OsRng, RngCore};
        for _ in 0..200 {
            let mut buf = vec![0u8; (OsRng.next_u32() % 400) as usize];
            OsRng.fill_bytes(&mut buf);
            let mut v = empty_vault();
            let _ = v.import_csv(&buf); // must not panic; any result is fine
        }
    }

    #[test]
    fn categories_are_guessed_from_well_known_sites() {
        assert_eq!(guess_category("facebook.com", ""), "social");
        assert_eq!(guess_category("My Bank", "URL: https://online.example"), "bank");
        assert_eq!(guess_category("Netflix", ""), "entertainment");
        assert_eq!(guess_category("dropbox.com", ""), "work");
        assert_eq!(guess_category("amazon.com", ""), "shopping");
        assert_eq!(guess_category("gmail.com", ""), "email");
        assert_eq!(guess_category("random-site.example", ""), "");
    }

    #[test]
    fn import_assigns_guessed_categories() {
        let csv = "name,url,username,password\n\
                   facebook.com,https://facebook.com,me,pw1\n\
                   random-site.example,,me,pw2\n";
        let mut v = empty_vault();
        v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(v.entries()[0].category, "social");
        assert_eq!(v.entries()[1].category, "");
    }

    #[test]
    fn bitwarden_two_factor_secrets_are_imported() {
        let csv = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
                   ,,login,Example,,,0,https://example.com,me,pw-one,GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\n\
                   ,,login,Broken,,,0,https://broken.example,me,pw-two,not a secret!!\n\
                   ,,login,Plain,,,0,https://plain.example,me,pw-three,\n";
        let mut v = empty_vault();
        let r = v.import_csv(csv.as_bytes()).unwrap();
        assert_eq!(r.added, 3); // the unreadable secret does not stop the login
        assert!(v.entries()[0].totp.is_some());
        assert!(v.entries()[1].totp.is_none());
        assert!(v.entries()[2].totp.is_none());
    }
}