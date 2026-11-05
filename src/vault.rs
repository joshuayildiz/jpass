//! Encrypted CSV vault: `Account,Email/Username/ID,Password` inside an age
//! passphrase-encrypted file. Plaintext only ever lives in memory.

use age::secrecy::SecretString;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Entry {
    /// All CSV columns; the first three are account, user, password.
    /// Extra columns (if any) are preserved untouched.
    pub fields: Vec<String>,
}

impl Entry {
    pub fn new(account: &str, user: &str, password: &str) -> Self {
        Entry { fields: vec![account.trim().into(), user.trim().into(), password.trim().into()] }
    }

    pub fn account(&self) -> &str {
        self.field(0)
    }

    pub fn user(&self) -> &str {
        self.field(1)
    }

    pub fn password(&self) -> &str {
        self.field(2)
    }

    pub fn set(&mut self, account: String, user: String, password: String) {
        self.ensure_three_columns();
        self.fields[0] = account.trim().to_owned();
        self.fields[1] = user.trim().to_owned();
        self.fields[2] = password.trim().to_owned();
    }

    fn field(&self, i: usize) -> &str {
        self.fields.get(i).map_or("", String::as_str)
    }

    fn ensure_three_columns(&mut self) {
        if self.fields.len() < 3 {
            self.fields.resize(3, String::new());
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vault {
    pub header: Option<Vec<String>>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Default, PartialEq)]
pub struct MergeReport {
    pub added: usize,
    pub duplicates: usize,
    /// Subset of `added`: same account+identifier as an existing entry, but a different password.
    pub changed_password: usize,
}

impl Vault {
    pub fn empty() -> Vault {
        Vault {
            header: Some(vec!["Account".into(), "Identifier".into(), "Password".into()]),
            entries: vec![],
        }
    }

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

    /// Sorted, trimmed, and padded so every column lines up in a monospace editor.
    pub fn serialize(&self) -> String {
        let cells = self.cells();
        let widths = column_widths(&cells);
        cells.iter().map(|row| aligned_line(row, &widths)).collect()
    }

    /// Add every entry of `other` that is not already present. A duplicate has
    /// the same account, identifier (both case-insensitively) and password.
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
            if self.entries.iter().any(same_login) {
                report.changed_password += 1;
            }
            self.entries.push(e.clone());
            report.added += 1;
        }
        report
    }
}

// ───────────────────────────── the file ─────────────────────────────

/// `~/pCloudDrive/pass.rage` on Linux, `~/pCloud Drive/pass.rage` on macOS:
/// the pCloud client mounts the drive under a different name on each.
pub fn default_path() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(DRIVE_DIR).join("pass.rage")
}

#[cfg(target_os = "macos")]
const DRIVE_DIR: &str = "pCloud Drive";
#[cfg(not(target_os = "macos"))]
const DRIVE_DIR: &str = "pCloudDrive";

/// Is the directory that holds the vault actually available?
///
/// On Linux `~/pCloudDrive` is a FUSE mount that pCloud creates *after* login;
/// before that it is an empty plain directory, and a vault written there would
/// be hidden under the mount and never uploaded. So on Linux we require a real
/// mount point; elsewhere a non-empty folder.
pub fn drive_ready(dir: &Path) -> bool {
    dev_ready() || mounted(dir)
}

/// Dev builds only (compiled out of release): UI testing against a scratch home.
#[cfg(debug_assertions)]
fn dev_ready() -> bool {
    std::env::var_os("JPASS_DEV_PASS").is_some() || std::env::var_os("JPASS_DEV_READY").is_some()
}

#[cfg(not(debug_assertions))]
fn dev_ready() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn mounted(dir: &Path) -> bool {
    let want = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let Ok(mounts) = fs::read_to_string("/proc/mounts") else { return false };
    mounts.lines().filter_map(mount_point).any(|mp| mp == want)
}

/// The second field of a `/proc/mounts` line; `\040` stands for a space.
#[cfg(target_os = "linux")]
fn mount_point(line: &str) -> Option<PathBuf> {
    Some(PathBuf::from(line.split_whitespace().nth(1)?.replace("\\040", " ")))
}

#[cfg(not(target_os = "linux"))]
fn mounted(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
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
pub fn save(path: &Path, vault: &Vault, pass: &SecretString, expected: Option<&Stamp>) -> Result<Stamp, String> {
    let current = stamp(path);
    check_unchanged(expected, current.as_ref())?;
    let armored = expected.is_some_and(|e| e.armored);
    let bytes = encrypt(&vault.serialize(), pass, armored)?;
    if current.is_some() {
        let _ = fs::copy(path, sibling(path, |name| format!("{name}.bak")));
    }
    write_atomically(path, &bytes)?;
    let mut st = stamp(path).ok_or("File missing after save")?;
    st.armored = armored;
    Ok(st)
}

fn check_unchanged(expected: Option<&Stamp>, current: Option<&Stamp>) -> Result<(), String> {
    match (expected, current) {
        (Some(e), Some(c)) if !e.same_file(c) => {
            Err("pass.rage changed on disk (synced from elsewhere). Nothing was saved; press Reload.".into())
        }
        (Some(_), None) => Err("pass.rage disappeared from disk. Nothing was saved.".into()),
        (None, Some(_)) => Err("pass.rage already exists. Nothing was saved.".into()),
        _ => Ok(()),
    }
}

fn sibling(path: &Path, name: impl FnOnce(&str) -> String) -> PathBuf {
    let own = path.file_name().and_then(|n| n.to_str()).unwrap_or("pass.rage");
    path.with_file_name(name(own))
}

/// Write to a temp file next to `path`, sync it, then rename it over `path`.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = sibling(path, |name| format!(".{name}.jpass-tmp"));
    let mut f = fs::File::create(&tmp).map_err(|e| format!("Cannot write temp file: {e}"))?;
    f.write_all(bytes).map_err(|e| format!("Write failed: {e}"))?;
    f.sync_all().map_err(|e| format!("Sync failed: {e}"))?;
    drop(f);
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Cannot replace {}: {e}", path.display())
    })
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
    pub fn same_file(&self, other: &Stamp) -> bool {
        self.modified == other.modified && self.len == other.len
    }
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    let m = fs::metadata(path).ok()?;
    Some(Stamp { modified: m.modified().ok(), len: m.len(), armored: false })
}

// ───────────────────────────── encryption ─────────────────────────────

/// scrypt work factor used when encrypting. Fixed (rather than auto-calibrated)
/// so files stay decryptable by the `age`/`rage` CLI, whose limit is 22.
const WORK_FACTOR: u8 = 20;
/// Highest work factor we are willing to wait for when decrypting.
const MAX_WORK_FACTOR: u8 = 26;
const ARMOR_PREFIX: &[u8] = b"-----BEGIN AGE ENCRYPTED FILE-----";

/// True if `bytes` look like an age file (binary or ASCII-armored).
pub fn is_age(bytes: &[u8]) -> bool {
    bytes.starts_with(b"age-encryption.org/") || bytes.starts_with(ARMOR_PREFIX)
}

pub fn decrypt_text(bytes: &[u8], pass: &SecretString) -> Result<String, String> {
    let plain = age::decrypt(&identity(pass), bytes).map_err(|e| match e {
        age::DecryptError::DecryptionFailed | age::DecryptError::KeyDecryptionFailed => {
            "Wrong passphrase (or file is corrupted)".to_string()
        }
        other => format!("Decrypt failed: {other}"),
    })?;
    String::from_utf8(plain).map_err(|_| "Decrypted data is not UTF-8 text".to_string())
}

/// Encrypt, then verify we can read back what we are about to write.
fn encrypt(plain: &str, pass: &SecretString, armored: bool) -> Result<Vec<u8>, String> {
    let rec = recipient(pass);
    let out = if armored {
        age::encrypt_and_armor(&rec, plain.as_bytes()).map(String::into_bytes)
    } else {
        age::encrypt(&rec, plain.as_bytes())
    }
    .map_err(|e| format!("Encrypt failed: {e}"))?;
    let back = age::decrypt(&identity(pass), &out).map_err(|e| format!("Self-check failed: {e}"))?;
    if back != plain.as_bytes() {
        return Err("Self-check failed: round trip mismatch. Nothing was saved.".into());
    }
    Ok(out)
}

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

// ───────────────────────────── writing CSV ─────────────────────────────

impl Vault {
    /// Quoted cells: the header first, then the entries sorted by account
    /// and identifier (case-insensitive).
    fn cells(&self) -> Vec<Vec<String>> {
        let mut entries = self.entries.clone();
        entries.sort_by_cached_key(|e| (e.account().trim().to_lowercase(), e.user().trim().to_lowercase()));
        let rows = self.header.iter().cloned().chain(entries.into_iter().map(|mut e| {
            e.ensure_three_columns();
            e.fields
        }));
        rows.map(|row| row.iter().map(|f| quote_field(f.trim())).collect()).collect()
    }
}

/// Per column, the widest `cell,` among the rows where that column is not the last.
fn column_widths(cells: &[Vec<String>]) -> Vec<usize> {
    let ncols = cells.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; ncols];
    for row in cells {
        for (j, cell) in row.iter().enumerate().take(row.len().saturating_sub(1)) {
            widths[j] = widths[j].max(cell.chars().count() + 1);
        }
    }
    widths
}

/// One line: every cell but the last padded to its column width.
fn aligned_line(row: &[String], widths: &[usize]) -> String {
    let mut line = String::new();
    for (j, cell) in row.iter().enumerate() {
        if j + 1 == row.len() {
            line.push_str(cell);
        } else {
            let cell = format!("{cell},");
            let pad = widths[j] + 1 - cell.chars().count();
            line.push_str(&cell);
            line.extend(std::iter::repeat_n(' ', pad));
        }
    }
    line.trim_end().to_string() + "\n"
}

/// Quote a field only when it must be (comma, quote or line break inside).
fn quote_field(f: &str) -> String {
    if f.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", f.replace('"', "\"\""))
    } else {
        f.to_owned()
    }
}

// ───────────────────────────── reading CSV ─────────────────────────────

/// Minimal CSV reader that tolerates padding: spaces before/after a field
/// (including before an opening quote) are ignored and every field is trimmed.
/// Records that are entirely empty are skipped.
fn parse_records(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    let mut records = Vec::new();
    let mut record = Vec::new();
    loop {
        record.push(read_field(&mut chars)?);
        if !end_of_record(&mut chars) {
            continue;
        }
        if record.iter().any(|f| !f.is_empty()) {
            records.push(std::mem::take(&mut record));
        }
        record.clear();
        if chars.peek().is_none() {
            return Ok(records);
        }
    }
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// One trimmed field, stopping before the `,` or line break that follows it.
fn read_field(chars: &mut Chars) -> Result<String, String> {
    skip_blanks(chars);
    if chars.peek() != Some(&'"') {
        return Ok(read_bare(chars));
    }
    chars.next();
    let field = read_quoted(chars)?;
    skip_blanks(chars);
    match chars.peek() {
        Some(&c) if !is_field_end(c) => Err(format!("CSV error: unexpected '{c}' after a quoted field")),
        _ => Ok(field.trim().to_owned()),
    }
}

fn read_bare(chars: &mut Chars) -> String {
    let mut field = String::new();
    while let Some(&c) = chars.peek()
        && !is_field_end(c)
    {
        field.push(c);
        chars.next();
    }
    field.trim().to_owned()
}

/// The rest of a quoted field, its opening quote already consumed; `""` is a literal quote.
fn read_quoted(chars: &mut Chars) -> Result<String, String> {
    let mut field = String::new();
    loop {
        match chars.next() {
            Some('"') if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            Some('"') => return Ok(field),
            Some(c) => field.push(c),
            None => return Err("CSV error: unterminated quoted field".into()),
        }
    }
}

/// Consume what follows a field: `,` continues the record; a line break or the end finishes it.
fn end_of_record(chars: &mut Chars) -> bool {
    match chars.next() {
        Some(',') => false,
        Some('\r') => {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            true
        }
        _ => true,
    }
}

fn is_field_end(c: char) -> bool {
    matches!(c, ',' | '\n' | '\r')
}

fn skip_blanks(chars: &mut Chars) {
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
    }
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

    /// `JPASS_ENC_IN=a.csv JPASS_ENC_OUT=b.rage cargo test make_encrypted -- --ignored`
    #[test]
    #[ignore]
    fn make_encrypted() {
        let text = std::fs::read_to_string(std::env::var("JPASS_ENC_IN").unwrap()).unwrap();
        let out = age::encrypt(&recipient(&sec("other")), text.as_bytes()).unwrap();
        std::fs::write(std::env::var("JPASS_ENC_OUT").unwrap(), out).unwrap();
    }
}
