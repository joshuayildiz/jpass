//! `jpass import`: merge a CSV file into the encrypted vault.

use crate::vault::{self, Vault};
use age::secrecy::SecretString;

/// `jpass import file.csv`: merge a CSV (plaintext or age-encrypted) into the encrypted vault.
pub fn main(file: &str) -> Result<(), String> {
    use age::secrecy::ExposeSecret;
    let raw = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let encrypted = vault::is_age(&raw);
    let path = vault::default_path();
    let prompt = |msg: &str| -> Result<SecretString, String> {
        rpassword::prompt_password(msg)
            .map(SecretString::from)
            .map_err(|e| format!("cannot read passphrase: {e}"))
    };

    // 1. The vault's own passphrase (or choose one if the vault is new).
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    if !vault::drive_ready(dir) {
        return Err(format!("{} is not mounted yet; wait for pCloud and retry", dir.display()));
    }
    let creating = !path.exists();
    let (existing, stamp, pass) = if creating {
        println!("{} does not exist yet; it will be created.", path.display());
        let pass = prompt("Choose a master passphrase: ")?;
        let again = prompt("Repeat passphrase: ")?;
        if pass.expose_secret() != again.expose_secret() {
            return Err("passphrases do not match".into());
        }
        (None, None, pass)
    } else {
        let pass = prompt(&format!("Master passphrase for {}: ", path.display()))?;
        let (v, st) = vault::load(&path, &pass)?;
        (Some(v), Some(st), pass)
    };

    // 2. The import file: plaintext CSV, or age-encrypted with its own passphrase.
    let text = if encrypted {
        let p = prompt(&format!("Passphrase for {file} (Enter = same as master): "))?;
        let p = if p.expose_secret().is_empty() { pass.clone() } else { p };
        vault::decrypt_text(&raw, &p)?
    } else {
        String::from_utf8(raw).map_err(|_| format!("{file} is not UTF-8 text"))?
    };
    let incoming = Vault::parse(&text)?;
    if incoming.entries.is_empty() {
        return Err(format!("{file} contains no entries"));
    }

    let mut v = existing.unwrap_or_else(|| Vault {
        header: incoming
            .header
            .clone()
            .or_else(|| Some(vec!["Account".into(), "Identifier".into(), "Password".into()])),
        entries: vec![],
    });
    let r = v.merge(&incoming);
    if r.added > 0 {
        vault::save(&path, &v, &pass, stamp.as_ref())?;
    }
    println!(
        "Imported {} new, skipped {} duplicate{}.",
        r.added,
        r.duplicates,
        if r.duplicates == 1 { "" } else { "s" }
    );
    if r.changed_password > 0 {
        println!(
            "Note: {} of the new entries have the same account + identifier as an existing entry \
             but a different password; both were kept.",
            r.changed_password
        );
    }
    if r.added > 0 {
        println!("Saved to {}.", path.display());
    }
    Ok(())
}
