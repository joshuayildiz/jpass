//! `jpass import file.csv`: merge a CSV (plaintext or age-encrypted) into the vault.

use crate::vault::{self, MergeReport, Stamp, Vault};
use age::secrecy::{ExposeSecret, SecretString};
use std::path::Path;

pub fn main(file: &str) -> Result<(), String> {
    let raw = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let path = vault::default_path();
    require_drive(&path)?;
    let (existing, stamp, pass) = open_vault(&path)?;
    let incoming = read_import(file, raw, &pass)?;
    let mut vault = existing.unwrap_or_else(|| empty_like(&incoming));
    let report = vault.merge(&incoming);
    if report.added > 0 {
        vault::save(&path, &vault, &pass, stamp.as_ref())?;
    }
    print_report(&report, &path);
    Ok(())
}

fn require_drive(path: &Path) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    if vault::drive_ready(dir) {
        return Ok(());
    }
    Err(format!("{} is not mounted yet; wait for pCloud and retry", dir.display()))
}

/// The vault and its passphrase; for a vault that does not exist yet, a freshly chosen passphrase.
fn open_vault(path: &Path) -> Result<(Option<Vault>, Option<Stamp>, SecretString), String> {
    if path.exists() {
        let pass = prompt(&format!("Master passphrase for {}: ", path.display()))?;
        let (vault, stamp) = vault::load(path, &pass)?;
        return Ok((Some(vault), Some(stamp), pass));
    }
    println!("{} does not exist yet; it will be created.", path.display());
    let pass = prompt("Choose a master passphrase: ")?;
    let again = prompt("Repeat passphrase: ")?;
    if pass.expose_secret() != again.expose_secret() {
        return Err("passphrases do not match".into());
    }
    Ok((None, None, pass))
}

/// The import file: plaintext CSV, or age-encrypted with its own passphrase.
fn read_import(file: &str, raw: Vec<u8>, master: &SecretString) -> Result<Vault, String> {
    let text = if vault::is_age(&raw) {
        let own = prompt(&format!("Passphrase for {file} (Enter = same as master): "))?;
        let pass = if own.expose_secret().is_empty() { master } else { &own };
        vault::decrypt_text(&raw, pass)?
    } else {
        String::from_utf8(raw).map_err(|_| format!("{file} is not UTF-8 text"))?
    };
    let incoming = Vault::parse(&text)?;
    if incoming.entries.is_empty() {
        return Err(format!("{file} contains no entries"));
    }
    Ok(incoming)
}

/// A new vault with the header of `incoming` (or the standard one) and no entries.
fn empty_like(incoming: &Vault) -> Vault {
    let header = incoming.header.clone().or_else(|| Vault::empty().header);
    Vault { header, entries: vec![] }
}

fn print_report(r: &MergeReport, path: &Path) {
    let plural = if r.duplicates == 1 { "" } else { "s" };
    println!("Imported {} new, skipped {} duplicate{plural}.", r.added, r.duplicates);
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
}

fn prompt(msg: &str) -> Result<SecretString, String> {
    rpassword::prompt_password(msg).map(SecretString::from).map_err(|e| format!("cannot read passphrase: {e}"))
}
