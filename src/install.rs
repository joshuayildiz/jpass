//! First-run / `install` setup: make jpass a proper desktop application that
//! starts at login, so its global hotkey is always live.
//!
//! * macOS:  `/Applications/jpass.app` (falls back to `~/Applications`) + a
//!           LaunchAgent that starts it hidden at login.
//! * Linux:  `~/.local/bin/jpass`, a `.desktop` launcher in
//!           `~/.local/share/applications` and an XDG autostart entry
//!           (works on CachyOS/Arch with KDE, GNOME, etc.).
//!
//! Everything is per-user (no root) and idempotent.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn marker_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("jpass").join("installed-v1"))
}

pub fn is_installed() -> bool {
    marker_path().is_some_and(|p| p.exists())
}

/// `jpass install`: (re)install and print what was done.
pub fn main() -> Result<(), String> {
    run()?.iter().for_each(|l| println!("{l}"));
    Ok(())
}

/// Install and remember that we did. Returns a human-readable log.
pub fn run() -> Result<Vec<String>, String> {
    let log = platform_install()?;
    if let Some(m) = marker_path() {
        if let Some(parent) = m.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&m, "1\n");
    }
    Ok(log)
}

/// Copy `src` to `dest` atomically (works even if `dest` is a running binary).
fn install_binary(src: &Path, dest: &Path) -> Result<Option<String>, String> {
    if let (Ok(a), Ok(b)) = (src.canonicalize(), dest.canonicalize()) {
        if a == b {
            return Ok(None); // already running from the installed location
        }
    }
    let dir = dest.parent().ok_or("bad destination")?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = dir.join(".jpass.new");
    fs::copy(src, &tmp).map_err(|e| format!("cannot copy binary to {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755));
    }
    fs::rename(&tmp, dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot install {}: {e}", dest.display())
    })?;
    Ok(Some(format!("installed {}", dest.display())))
}

fn write_file(path: &Path, contents: &str) -> Result<String, String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(format!("wrote {}", path.display()))
}

// ───────────────────────────── Linux ─────────────────────────────

#[cfg(not(target_os = "macos"))]
fn platform_install() -> Result<Vec<String>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))?;
    let home = dirs::home_dir().ok_or("no home directory")?;
    let bin = home.join(".local/bin/jpass");
    let mut log = Vec::new();

    log.extend(install_binary(&exe, &bin)?);
    let bin_s = bin.display().to_string();

    let launcher = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=jpass\n\
         GenericName=Password Manager\n\
         Comment=Search and copy passwords (show/hide)\n\
         Exec={bin_s} toggle\n\
         Icon=dialog-password\n\
         Terminal=false\n\
         Categories=Utility;Security;\n\
         Keywords=password;passwords;vault;age;\n"
    );
    let apps = home.join(".local/share/applications");
    log.push(write_file(&apps.join("jpass.desktop"), &launcher)?);

    // Start hidden at login so the global hotkey works immediately.
    let autostart = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=jpass\n\
         Comment=jpass background service (global hotkey)\n\
         Exec={bin_s} hidden\n\
         Icon=dialog-password\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n\
         X-KDE-autostart-after=panel\n"
    );
    let ac = dirs::config_dir().ok_or("no config directory")?.join("autostart");
    log.push(write_file(&ac.join("jpass.desktop"), &autostart)?);

    // Best effort: refresh desktop caches so the launcher shows up right away.
    let _ = Command::new("update-desktop-database").arg(&apps).output();
    let _ = Command::new("kbuildsycoca6").output();

    log.push("hotkey: Alt+Space (the Cmd key) while jpass runs in the background; \
              or bind `jpass toggle` in your desktop's shortcut settings"
        .into());
    Ok(log)
}

// ───────────────────────────── macOS ─────────────────────────────

#[cfg(target_os = "macos")]
fn platform_install() -> Result<Vec<String>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))?;
    let home = dirs::home_dir().ok_or("no home directory")?;
    let mut log = Vec::new();

    // Prefer /Applications; fall back to ~/Applications if not writable.
    let mut app = PathBuf::from("/Applications/jpass.app");
    let bin_rel = "Contents/MacOS/jpass";
    let mut res = install_binary(&exe, &app.join(bin_rel));
    if res.is_err() {
        app = home.join("Applications/jpass.app");
        res = install_binary(&exe, &app.join(bin_rel));
    }
    log.extend(res?);

    let plist = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
        <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
        <plist version=\"1.0\"><dict>\n\
        <key>CFBundleName</key><string>jpass</string>\n\
        <key>CFBundleDisplayName</key><string>jpass</string>\n\
        <key>CFBundleIdentifier</key><string>com.jpass.app</string>\n\
        <key>CFBundleExecutable</key><string>jpass</string>\n\
        <key>CFBundlePackageType</key><string>APPL</string>\n\
        <key>CFBundleVersion</key><string>1</string>\n\
        <key>CFBundleShortVersionString</key><string>0.1.0</string>\n\
        <key>LSMinimumSystemVersion</key><string>11.0</string>\n\
        <key>LSUIElement</key><true/>\n\
        <key>NSHighResolutionCapable</key><true/>\n\
        </dict></plist>\n";
    log.push(write_file(&app.join("Contents/Info.plist"), plist)?);

    // LaunchAgent: start hidden at login so Cmd+Space is always live.
    let bin = app.join(bin_rel).display().to_string();
    let agent = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>com.jpass.app</string>\n\
         <key>ProgramArguments</key><array><string>{bin}</string><string>hidden</string></array>\n\
         <key>RunAtLoad</key><true/>\n\
         <key>KeepAlive</key><false/>\n\
         <key>ProcessType</key><string>Interactive</string>\n\
         </dict></plist>\n"
    );
    let agent_path = home.join("Library/LaunchAgents/com.jpass.app.plist");
    log.push(write_file(&agent_path, &agent)?);

    // Best effort: ad-hoc sign so Gatekeeper lets the copied binary run, and
    // register the login item for this session.
    let _ = Command::new("codesign").args(["--force", "--deep", "-s", "-"]).arg(&app).output();
    let _ = Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&app).output();
    if let Ok(o) = Command::new("id").arg("-u").output() {
        let uid = String::from_utf8_lossy(&o.stdout).trim().to_string();
        let _ = Command::new("launchctl")
            .args(["bootstrap", &format!("gui/{uid}")])
            .arg(&agent_path)
            .output();
    }

    log.push("hotkey: Cmd+Space while jpass runs in the background (disable Spotlight's \
              Cmd+Space in System Settings → Keyboard → Keyboard Shortcuts → Spotlight)"
        .into());
    Ok(log)
}
