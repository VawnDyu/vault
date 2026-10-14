// Small command-line tester for vault_core (no UI yet).
//
//   cargo run -- init            create my.vault
//   cargo run -- add SITE USER   add an entry (asks for the entry's password)
//   cargo run -- list            list entries (passwords hidden)
//   cargo run -- show ID         show one entry including its password
//   cargo run -- delete ID       delete an entry

use std::{env, path::Path, process};
use vault_core::generator::{self, GeneratorOptions};
use vault_core::{KdfParams, Vault, VaultError};

const FILE: &str = "my.vault";

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");
    if let Err(e) = run(cmd, &args) {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

fn unlock() -> Result<Vault, VaultError> {
    let pw = rpassword::prompt_password("Master password: ")?;
    Vault::open_file(FILE, &pw)
}

fn parse_id(args: &[String]) -> Result<u64, VaultError> {
    args.get(2)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| VaultError::Internal("please give a numeric ID".into()))
}

fn run(cmd: &str, args: &[String]) -> Result<(), VaultError> {
    match cmd {
        "init" => {
            if Path::new(FILE).exists() {
                return Err(VaultError::Internal(format!("{FILE} already exists")));
            }
            let pw = rpassword::prompt_password("Choose master password: ")?;
            let again = rpassword::prompt_password("Repeat master password: ")?;
            if pw != again {
                return Err(VaultError::Internal("passwords do not match".into()));
            }
            Vault::create(&pw, KdfParams::default())?.save(FILE)?;
            println!("Created {FILE}");
        }
        "add" => {
            let (site, user) = match (args.get(2), args.get(3)) {
                (Some(s), Some(u)) => (s, u),
                _ => return Err(VaultError::Internal("usage: add SITE USER".into())),
            };
            let mut vault = unlock()?;
            let pw = rpassword::prompt_password("Password for this entry: ")?;
            let id = vault.add_entry(site, user, &pw, "");
            vault.save(FILE)?;
            println!("Added entry #{id}");
        }
        "list" => {
            let vault = unlock()?;
            if vault.entries().is_empty() {
                println!("(vault is empty)");
            }
            for e in vault.entries() {
                println!("#{}  {}  ({})", e.id, e.site, e.username);
            }
        }
        "show" => {
            let id = parse_id(args)?;
            let vault = unlock()?;
            let e = vault.get_entry(id)?;
            println!("Site: {}\nUser: {}\nPassword: {}\nNotes: {}", e.site, e.username, e.password, e.notes);
        }
        "delete" => {
            let id = parse_id(args)?;
            let mut vault = unlock()?;
            vault.delete_entry(id)?;
            vault.save(FILE)?;
            println!("Deleted #{id}");
        }
        "gen" => {
            let mut opts = GeneratorOptions::default();
            if let Some(n) = args.get(2).and_then(|s| s.parse().ok()) {
                opts.length = n;
            }
            let pw = generator::generate(&opts)
                .map_err(|e| VaultError::Internal(e.to_string()))?;
            println!("{pw}");
            println!("(about {:.0} bits of entropy)", generator::entropy_bits(&opts));
        }
        _ => println!("Commands: init | add SITE USER | list | show ID | delete ID | gen [LENGTH]"),
    }
    Ok(())
}