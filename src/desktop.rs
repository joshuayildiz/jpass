//! Linux desktop integration shared by the launcher and the installer:
//! session D-Bus calls (through `busctl`, present on every systemd system)
//! and KDE's `kdeglobals` settings file.

use std::process::Command;

/// Call `method` on the session bus. `args` is the busctl signature followed
/// by the values (empty for a method without arguments). Returns busctl's
/// rendering of the reply, e.g. `b true` or `ai 1 134217760`.
pub fn dbus(dest: &str, path: &str, iface: &str, method: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("busctl")
        .args(["--user", "--timeout=2", "call", dest, path, iface, method])
        .args(args)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `gsettings get schema key`, without the quotes around a string value.
pub fn gsettings(schema: &str, key: &str) -> Option<String> {
    let out = Command::new("gsettings").args(["get", schema, key]).output().ok()?;
    let value = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string();
    (out.status.success() && !value.is_empty()).then_some(value)
}

/// `key` in `[section]` of `~/.config/kdeglobals`, if set.
pub fn kdeglobals(section: &str, key: &str) -> Option<String> {
    let cfg = std::fs::read_to_string(dirs::config_dir()?.join("kdeglobals")).ok()?;
    let header = format!("[{section}]");
    let mut in_section = false;
    for line in cfg.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == header;
        } else if in_section
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
            && !v.trim().is_empty()
        {
            return Some(v.trim().to_string());
        }
    }
    None
}
