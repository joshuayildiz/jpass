//! Encrypted CSV vault: `Account,Email/Username/ID,Password` inside an age
//! passphrase-encrypted file. Plaintext only ever lives in memory.

use age::secrecy::SecretString;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// scrypt work factor used when encrypting. Fixed (rather than auto-calibrated)
/// so files stay decryptable by the `age`/`rage` CLI, whose limit is 22.
const WORK_FACTOR: u8 = 20;
/// Highest work factor we are willing to wait for when decrypting.
const MAX_WORK_FACTOR: u8 = 26;

fn identity(pass: &SecretString) -> age::scrypt::Identity {
    let mut id = age::scrypt::Identity::new(pass.clone());
    id.set_max_work_factor(MAX_WORK_FACTOR);
    id
}

fn recipient(pass: &SecretString) -> age::scrypt::Recipient {
    let mut r = age::scrypt::Recipient::new(pass.clone());
    r.set_work_factor(WORK_FACTOR);
    r
}

const ARMOR_PREFIX: &[u8] = b"-----BEGIN AGE ENCRYPTED FILE-----";

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Entry {
    /// All CSV columns; the first three are account, user, password.
    /// Extra columns (if any) are preserved untouched.
    pub fields: Vec<String>,
}

impl Entry {
    pub fn new(account: &str, user: &str, password: &str) -> Self {
        Entry {
            fields: vec![account.trim().into(), user.trim().into(), password.trim().into()],
        }
    }
    fn ensure(&mut self) {
        while self.fields.len() < 3 {
            self.fields.push(String::new());
        }
    }
    pub fn account(&self) -> &str {
        self.fields.first().map_or("", |s| s)
    }
    pub fn user(&self) -> &str {
        self.fields.get(1).map_or("", |s| s)
    }
    pub fn password(&self) -> &str {
        self.fields.get(2).map_or("", |s| s)
    }
    pub fn set(&mut self, account: String, user: String, password: String) {
        self.ensure();
        self.fields[0] = account.trim().to_owned();
        self.fields[1] = user.trim().to_owned();
        self.fields[2] = password.trim().to_owned();
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct MergeReport {
    pub added: usize,
    pub duplicates: usize,
    /// Subset of `added`: same account+identifier as an existing entry, but a different password.
    pub changed_password: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vault {
    pub header: Option<Vec<String>>,
    pub entries: Vec<Entry>,
}

impl Vault {
    /// Parse the CSV. Fields are trimmed, padding around fields (as written
    /// by `serialize`) is ignored, and blank lines are skipped.
    pub fn parse(text: &str) -> Result<Vault, String> {
        let mut header = None;
        let mut entries = Vec::new();
        for (i, fields) in parse_records(text)?.into_iter().enumerate() {
            if i == 0 && fields[0].eq_ignore_ascii_case("account") {
                header = Some(fields);
                continue;
            }
            entries.push(Entry { fields });
        }
        Ok(Vault { header, entries })
    }

    /// Add every entry of `other` that is not already present.
    /// An entry is a duplicate when account + identifier + password all match
    /// (account and identifier compared case-insensitively, trimmed).
    pub fn merge(&mut self, other: &Vault) -> MergeReport {
        let norm = |s: &str| s.trim().to_lowercase();
        let mut report = MergeReport::default();
        for e in &other.entries {
            let (acc, usr) = (norm(e.account()), norm(e.user()));
            let same_login = |x: &Entry| norm(x.account()) == acc && norm(x.user()) == usr;
            if self.entries.iter().any(|x| same_login(x) && x.password() == e.password()) {
                report.duplicates += 1;
                continue;
            }
            if self.entries.iter().any(|x| same_login(x)) {
                report.changed_password += 1; // same login, different password: keep both
            }
            self.entries.push(e.clone());
            report.added += 1;
        }
        report
    }

    /// Render the file: sorted, trimmed, and padded so that every column
    /// starts at the same horizontal position in a monospace editor.
    pub fn serialize(&self) -> String {
        let mut rows: Vec<Vec<String>> = Vec::new();
        if let Some(h) = &self.header {
            rows.push(h.clone());
        }
        // Sorted by account, then identifier (case-insensitive); the header stays first.
        let mut sorted: Vec<Entry> = self.entries.clone();
        sorted.sort_by_cached_key(|e| (e.account().trim().to_lowercase(), e.user().trim().to_lowercase()));
        for mut e in sorted {
            e.ensure();
            rows.push(e.fields);
        }
        let cells: Vec<Vec<String>> =
            rows.iter().map(|r| r.iter().map(|f| quote_field(f.trim())).collect()).collect();

        // width[j] = widest "cell," among rows where column j is not the last one.
        let ncols = cells.iter().map(Vec::len).max().unwrap_or(0);
        let mut width = vec![0usize; ncols];
        for r in &cells {
            for (j, c) in r.iter().enumerate().take(r.len().saturating_sub(1)) {
                width[j] = width[j].max(c.chars().count() + 1);
            }
        }
        let mut out = String::new();
        for r in &cells {
            let mut line = String::new();
            for (j, c) in r.iter().enumerate() {
                if j + 1 < r.len() {
                    let cell = format!("{c},");
                    let pad = width[j] - cell.chars().count();
                    line.push_str(&cell);
                    line.extend(std::iter::repeat_n(' ', pad + 1));
                } else {
                    line.push_str(c);
                }
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

/// Quote a field only when it must be (comma, quote or line break inside).
fn quote_field(f: &str) -> String {
    if f.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", f.replace('"', "\"\""))
    } else {
        f.to_owned()
    }
}

/// Minimal CSV reader that tolerates padding: spaces before/after a field
/// (including before an opening quote) are ignored and every field is trimmed.
/// Records that are entirely empty are skipped.
fn parse_records(text: &str) -> Result<Vec<Vec<String>>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut chars = text.chars().peekable();
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let skip_blanks = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
        while matches!(it.peek(), Some(' ' | '\t')) {
            it.next();
        }
    };
    loop {
        skip_blanks(&mut chars);
        let mut field = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') if chars.peek() == Some(&'"') => {
                        chars.next();
                        field.push('"');
                    }
                    Some('"') => break,
                    Some(c) => field.push(c),
                    None => return Err("CSV error: unterminated quoted field".into()),
                }
            }
            skip_blanks(&mut chars);
            if let Some(&c) = chars.peek() {
                if !matches!(c, ',' | '\n' | '\r') {
                    return Err(format!("CSV error: unexpected '{c}' after a quoted field"));
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if matches!(c, ',' | '\n' | '\r') {
                    break;
                }
                field.push(c);
                chars.next();
            }
        }
        record.push(field.trim().to_owned());
        let end_of_record = match chars.next() {
            Some(',') => false,
            Some('\r') => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                true
            }
            Some('\n') | None => true,
            Some(_) => unreachable!("field parsing stops only at , \\n \\r or end"),
        };
        if end_of_record {
            if record.iter().any(|f| !f.is_empty()) {
                records.push(std::mem::take(&mut record));
            } else {
                record.clear();
            }
            if chars.peek().is_none() {
                break;
            }
        }
    }
    Ok(records)
}

/// Identifies the on-disk file version we loaded, so we never clobber a newer
/// version that pCloud synced in from another machine.
#[derive(Clone, Debug, PartialEq)]
pub struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    armored: bool,
}

impl Stamp {
    /// Same file version on disk (ignores the armor flag).
    pub fn same_file(&self, other: &Stamp) -> bool {
        self.modified == other.modified && self.len == other.len
    }
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    let m = fs::metadata(path).ok()?;
    Some(Stamp {
        modified: m.modified().ok(),
        len: m.len(),
        armored: false,
    })
}

/// Is the directory that holds the vault actually available?
///
/// `~/pCloudDrive` is a FUSE mount that pCloud creates *after* login; before
/// that it is an empty plain directory, and writing a vault there would hide
/// it under the mount and never upload it. On Linux we therefore require a
/// real mount point; elsewhere we require the folder to be non-empty.
pub fn drive_ready(dir: &Path) -> bool {
    // Dev builds only (compiled out of release): UI testing against a scratch home.
    #[cfg(debug_assertions)]
    if std::env::var_os("JPASS_DEV_PASS").is_some() || std::env::var_os("JPASS_DEV_READY").is_some() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        let want = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        return fs::read_to_string("/proc/mounts").is_ok_and(|m| {
            m.lines().any(|l| {
                l.split_whitespace()
                    .nth(1)
                    .is_some_and(|mp| Path::new(&mp.replace("\\040", " ")) == want)
            })
        });
    }
    #[cfg(not(target_os = "linux"))]
    {
        fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
    }
}

/// `~/pCloudDrive/pass.rage` on Linux, `~/pCloud Drive/pass.rage` on macOS:
/// the pCloud client mounts the drive under a different name on each.
pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DRIVE_DIR)
        .join("pass.rage")
}

#[cfg(target_os = "macos")]
const DRIVE_DIR: &str = "pCloud Drive";
#[cfg(not(target_os = "macos"))]
const DRIVE_DIR: &str = "pCloudDrive";

/// True if `bytes` look like an age file (binary or ASCII-armored).
pub fn is_age(bytes: &[u8]) -> bool {
    bytes.starts_with(b"age-encryption.org/") || bytes.starts_with(ARMOR_PREFIX)
}

/// Decrypt passphrase-encrypted age bytes to UTF-8 text.
pub fn decrypt_text(bytes: &[u8], pass: &SecretString) -> Result<String, String> {
    let plain = age::decrypt(&identity(pass), bytes).map_err(|e| match e {
        age::DecryptError::DecryptionFailed | age::DecryptError::KeyDecryptionFailed => {
            "Wrong passphrase (or file is corrupted)".to_string()
        }
        other => format!("Decrypt failed: {other}"),
    })?;
    String::from_utf8(plain).map_err(|_| "Decrypted data is not UTF-8 text".to_string())
}

pub fn load(path: &Path, pass: &SecretString) -> Result<(Vault, Stamp), String> {
    let before = stamp(path);
    let bytes = fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let vault = Vault::parse(&decrypt_text(&bytes, pass)?)?;
    let mut st = before.ok_or("File vanished while reading")?;
    st.armored = bytes.starts_with(ARMOR_PREFIX);
    Ok((vault, st))
}

/// Encrypt and atomically replace the file. `expected` is the stamp from the
/// last load/save; if the file changed since, we refuse. `None` means
/// "creating a new file" (refuse if one now exists).
pub fn save(
    path: &Path,
    vault: &Vault,
    pass: &SecretString,
    expected: Option<&Stamp>,
) -> Result<Stamp, String> {
    let current = stamp(path);
    match (expected, &current) {
        (Some(e), Some(c)) if e.modified != c.modified || e.len != c.len => {
            return Err("pass.rage changed on disk (synced from elsewhere). Nothing was saved; \
                        press Reload."
                .into());
        }
        (Some(_), None) => return Err("pass.rage disappeared from disk. Nothing was saved.".into()),
        (None, Some(_)) => return Err("pass.rage already exists. Nothing was saved.".into()),
        _ => {}
    }
    let armored = expected.is_some_and(|e| e.armored);
    let plain = vault.serialize();
    let rec = recipient(pass);
    let out: Vec<u8> = if armored {
        age::encrypt_and_armor(&rec, plain.as_bytes())
            .map_err(|e| format!("Encrypt failed: {e}"))?
            .into_bytes()
    } else {
        age::encrypt(&rec, plain.as_bytes()).map_err(|e| format!("Encrypt failed: {e}"))?
    };

    // Verify we can read back what we are about to write.
    let id = identity(pass);
    let back = age::decrypt(&id, &out).map_err(|e| format!("Self-check failed: {e}"))?;
    if back != plain.as_bytes() {
        return Err("Self-check failed: round trip mismatch. Nothing was saved.".into());
    }

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("pass.rage");
    let tmp = dir.join(format!(".{name}.jpass-tmp"));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("Cannot write temp file: {e}"))?;
        f.write_all(&out).map_err(|e| format!("Write failed: {e}"))?;
        f.sync_all().map_err(|e| format!("Sync failed: {e}"))?;
    }
    if current.is_some() {
        let bak = dir.join(format!("{name}.bak"));
        let _ = fs::copy(path, &bak);
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Cannot replace {}: {e}", path.display())
    })?;
    let mut st = stamp(path).ok_or("File missing after save")?;
    st.armored = armored;
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(s: &str) -> SecretString {
        SecretString::from(s.to_string())
    }

    #[test]
    fn roundtrip_keeps_header_quotes_and_extra_columns() {
        let text = "Account,Email/Username/ID,Password\n\
                    \"Bank, Inc\",\"a\"\"b\",\"p,w\"\n\
                    Extra,u,p,note\n\
                    GitHub,me@x.com,hunter2\n";
        let v = Vault::parse(text).unwrap();
        assert!(v.header.is_some());
        assert_eq!(v.entries.len(), 3);
        assert_eq!(v.entries[0].account(), "Bank, Inc");
        assert_eq!(v.entries[0].user(), "a\"b");
        assert_eq!(v.entries[0].password(), "p,w");
        assert_eq!(v.entries[1].fields.len(), 4);
        // what we write must parse back to exactly the same data
        let out = v.serialize();
        assert_eq!(Vault::parse(&out).unwrap(), v);
        // and writing is idempotent
        assert_eq!(Vault::parse(&out).unwrap().serialize(), out);
    }

    #[test]
    fn merge_skips_duplicates_and_keeps_changed_passwords() {
        let mut v = Vault::parse("Account,Identifier,Password\nGitHub,me@x.com,pw1\nGmail,me@x.com,pw2\n").unwrap();
        let o = Vault::parse(
            "Account,Email/Username/ID,Password\n\
             github , ME@x.com,pw1\n\
             Gmail,me@x.com,NEW\n\
             Steam,me,pw3\n\
             Steam,me,pw3\n",
        )
        .unwrap();
        let r = v.merge(&o);
        assert_eq!(r, MergeReport { added: 2, duplicates: 2, changed_password: 1 });
        assert_eq!(v.entries.len(), 4);
        assert_eq!(v.merge(&o).added, 0, "second import adds nothing");
    }

    #[test]
    fn saved_file_is_sorted_case_insensitively() {
        let v = Vault::parse(
            "Account,Identifier,Password\nzeta,a,1\nAlpha,z,2\nalpha,B,3\nBeta,x,4\n",
        )
        .unwrap();
        let accounts: Vec<String> = v
            .serialize()
            .lines()
            .skip(1)
            .map(|l| l.split(',').next().unwrap().to_owned())
            .collect();
        assert_eq!(accounts, ["alpha", "Alpha", "Beta", "zeta"]);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn drive_ready_requires_a_real_mount_point() {
        assert!(drive_ready(Path::new("/proc")), "/proc is a mount point");
        let plain = std::env::temp_dir().join(format!("jpass-nomount-{}", std::process::id()));
        fs::create_dir_all(&plain).unwrap();
        assert!(!drive_ready(&plain), "an ordinary directory must not count");
        fs::remove_dir_all(&plain).unwrap();
    }

    #[test]
    fn headerless_file_stays_headerless() {
        let v = Vault::parse("a,b,c\n").unwrap();
        assert!(v.header.is_none());
        assert_eq!(v.serialize(), "a, b, c\n");
    }

    #[test]
    fn columns_are_aligned_for_monospace_editors() {
        let v = Vault::parse(
            "Account,Identifier,Password\nGmail,joshua@gmail.com,s3cret!\nGitHub,me@x.com,hunter2\n",
        )
        .unwrap();
        assert_eq!(
            v.serialize(),
            "Account, Identifier,       Password\n\
             GitHub,  me@x.com,         hunter2\n\
             Gmail,   joshua@gmail.com, s3cret!\n"
        );
    }

    #[test]
    fn fields_are_trimmed_and_padded_quotes_parse() {
        let v = Vault::parse(
            "Account  ,   Identifier ,  Password\n\
             \n\
             \"Bank, Inc\" ,   \"a\"\"b\"  ,   \"p,w\"   \n\
             \t GitHub \t,  me@x.com  ,hunter2  \r\n",
        )
        .unwrap();
        assert_eq!(v.header.as_deref(), Some(&["Account".to_string(), "Identifier".into(), "Password".into()][..]));
        assert_eq!(v.entries.len(), 2);
        assert_eq!(v.entries[0], Entry::new("Bank, Inc", "a\"b", "p,w"));
        assert_eq!(v.entries[1], Entry::new("GitHub", "me@x.com", "hunter2"));
        // new entries from the UI are trimmed too
        assert_eq!(Entry::new("  x ", " y", "z  "), Entry::new("x", "y", "z"));
    }

    #[test]
    fn encrypt_decrypt_and_conflict_detection() {
        let dir = std::env::temp_dir().join(format!("jpass-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pass.rage");
        let mut v = Vault::parse("Account,Email/Username/ID,Password\nA,b,c\n").unwrap();
        let p = sec("correct horse");
        let st = save(&path, &v, &p, None).unwrap();
        assert!(save(&path, &v, &p, None).is_err(), "must not overwrite on create");

        assert!(load(&path, &sec("wrong")).is_err());
        let (v2, st2) = load(&path, &p).unwrap();
        assert_eq!(v, v2);
        assert_eq!(st, st2);

        v.entries.push(Entry::new("D", "e", "f"));
        let st3 = save(&path, &v, &p, Some(&st2)).unwrap();
        assert!(dir.join("pass.rage.bak").exists());
        // a stale stamp must be rejected
        std::thread::sleep(std::time::Duration::from_millis(20));
        v.entries.push(Entry::new("G", "h", "i"));
        let st4 = save(&path, &v, &p, Some(&st3)).unwrap();
        assert!(save(&path, &v, &p, Some(&st3)).is_err());
        let (v5, _) = load(&path, &p).unwrap();
        assert_eq!(v5.entries.len(), 3);
        let _ = st4;
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Writes a sample vault for manual UI testing: `JPASS_SAMPLE=/path cargo test make_sample -- --ignored`
    #[test]
    #[ignore]
    fn make_sample() {
        let path = std::path::PathBuf::from(std::env::var("JPASS_SAMPLE").unwrap());
        let _ = fs::remove_file(&path);
        let text = "Account,Identifier,Password\nGitHub,joshua@example.com,hunter2\nGmail,joshua@gmail.com,s3cret!\nHetzner,joshua,pa55word\nNetflix,family@example.com,ilovecats\nAmazon,joshua@example.com,boxes123\nPCloud,joshua@example.com,clouds\nSteam,joshua_gamer,gg-ez\nProtonMail,joshua@proton.me,encrypted\n";
        let v = Vault::parse(text).unwrap();
        save(&path, &v, &sec("test"), None).unwrap();
    }
}
#[cfg(test)]
mod dev_helpers {
    use super::*;
    /// `JPASS_ENC_IN=a.csv JPASS_ENC_OUT=b.rage cargo test make_encrypted -- --ignored`
    #[test]
    #[ignore]
    fn make_encrypted() {
        let text = std::fs::read_to_string(std::env::var("JPASS_ENC_IN").unwrap()).unwrap();
        let out = age::encrypt(&recipient(&SecretString::from("other".to_string())), text.as_bytes()).unwrap();
        std::fs::write(std::env::var("JPASS_ENC_OUT").unwrap(), out).unwrap();
    }
}
