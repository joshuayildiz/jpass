//! First-run / `install` setup: make jpass a proper desktop application that
//! starts at login, so its global hotkey is always live. Everything is
//! per-user (no root) and idempotent.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// `jpass install`: (re)install and print what was done.
pub fn main() -> Result<(), String> {
    run()?.iter().for_each(|l| println!("{l}"));
    Ok(())
}

pub fn is_installed() -> bool {
    marker_path().is_some_and(|p| p.exists())
}

/// Install and remember that we did. Returns a human-readable log.
pub fn run() -> Result<Vec<String>, String> {
    let log = platform_install()?;
    remember_installed();
    Ok(log)
}

fn marker_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("jpass").join("installed-v1"))
}

fn remember_installed() {
    let Some(marker) = marker_path() else { return };
    if let Some(dir) = marker.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&marker, "1\n");
}

// ───────────────────────────── Linux ─────────────────────────────
// `~/.local/bin/jpass`, a `.desktop` launcher, an XDG autostart entry and,
// on KDE, a global shortcut.

#[cfg(not(target_os = "macos"))]
fn platform_install() -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("no home directory")?;
    let bin = home.join(".local/bin/jpass");
    let apps = home.join(".local/share/applications");
    let autostart = dirs::config_dir().ok_or("no config directory")?.join("autostart");
    let mut log = Vec::new();
    log.extend(install_binary(&own_binary()?, &bin)?);
    log.push(write_file(&apps.join("jpass.desktop"), &launcher_entry(&bin))?);
    log.push(write_file(&autostart.join("jpass.desktop"), &autostart_entry(&bin))?);
    refresh_desktop_caches(&apps);
    log.push(hotkey_note().into());
    Ok(log)
}

/// The application-menu entry; `jpass toggle` is what a KDE shortcut runs too.
#[cfg(not(target_os = "macos"))]
fn launcher_entry(bin: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=jpass\n\
         GenericName=Password Manager\n\
         Comment=Search and copy passwords (show/hide)\n\
         Exec={} toggle\n\
         Icon=dialog-password\n\
         Terminal=false\n\
         StartupNotify=false\n\
         Categories=Utility;Security;\n\
         Keywords=password;passwords;vault;age;\n",
        bin.display()
    )
}

/// Start hidden at login so the global hotkey works immediately.
#[cfg(not(target_os = "macos"))]
fn autostart_entry(bin: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=jpass\n\
         Comment=jpass background service (global hotkey)\n\
         Exec={} hidden\n\
         Icon=dialog-password\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n\
         X-KDE-autostart-after=panel\n",
        bin.display()
    )
}

/// Best effort: so the launcher shows up in the menus right away.
#[cfg(not(target_os = "macos"))]
fn refresh_desktop_caches(apps: &Path) {
    let _ = Command::new("update-desktop-database").arg(apps).output();
    let _ = Command::new("kbuildsycoca6").output();
}

#[cfg(not(target_os = "macos"))]
fn hotkey_note() -> &'static str {
    match kde_global_shortcut() {
        Some(true) => "hotkey: Alt+Space (KDE global shortcut; change it in System Settings → Shortcuts → jpass)",
        Some(false) => {
            "hotkey: registered with KDE, but Alt+Space is taken or unset — pick a key in \
             System Settings → Shortcuts → jpass"
        }
        None => {
            "hotkey: Alt+Space (the Cmd key) while jpass runs in the background (X11); \
             on other Wayland desktops bind `jpass toggle` to a key in the shortcut settings"
        }
    }
}

/// KDE: bind Alt+Space to `jpass.desktop` (its `Exec` is `jpass toggle`) through
/// kglobalaccel. The compositor consumes such a shortcut, so the focused app never
/// sees the key — unlike an X11 grab under XWayland. A key the user picked in
/// System Settings wins (`SetPresent` only, no `NoAutoloading`).
///
/// `None`: no KDE daemon. `Some(true)`: Alt+Space is bound. `Some(false)`: KDE
/// owns the hotkey but another (or no) key is set.
#[cfg(not(target_os = "macos"))]
pub fn kde_global_shortcut() -> Option<bool> {
    let register: Vec<&str> = ["as"].into_iter().chain(KDE_ACTION).collect();
    kglobalaccel("doRegister", &register)?;
    let set: Vec<&str> = ["asaiu"].into_iter().chain(KDE_ACTION).chain(["1", KDE_ALT_SPACE, "2"]).collect();
    let keys = kglobalaccel("setShortcut", &set)?;
    Some(keys.split_whitespace().any(|k| k == KDE_ALT_SPACE))
}

/// busctl `as`: count, then the kglobalaccel action id (component, action, their display names).
#[cfg(not(target_os = "macos"))]
const KDE_ACTION: [&str; 5] = ["4", "jpass.desktop", "_launch", "jpass", "Show or hide jpass"];
/// `Qt::ALT | Qt::Key_Space`
#[cfg(not(target_os = "macos"))]
const KDE_ALT_SPACE: &str = "134217760";

#[cfg(not(target_os = "macos"))]
fn kglobalaccel(method: &str, args: &[&str]) -> Option<String> {
    crate::desktop::dbus("org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel", method, args)
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    /// Live KDE session only.
    #[test]
    #[ignore]
    fn kde_shortcut_registers() {
        assert!(super::kde_global_shortcut().is_some());
    }
}

// ───────────────────────────── macOS ─────────────────────────────
// `/Applications/jpass.app` (or `~/Applications`) and a LaunchAgent that
// starts it hidden at login.

#[cfg(target_os = "macos")]
fn platform_install() -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("no home directory")?;
    let agent_path = home.join("Library/LaunchAgents/com.jpass.app.plist");
    let mut log = Vec::new();
    let app = install_bundle(&own_binary()?, &home, &mut log)?;
    log.push(write_file(&app.join("Contents/Info.plist"), INFO_PLIST)?);
    let log_file = home.join("Library/Logs/jpass.log");
    log.push(write_file(&agent_path, &launch_agent(&app.join(BUNDLE_BIN), &log_file))?);
    sign_bundle(&app);
    log.push(reload_agent(&agent_path)?);
    log.push(HOTKEY_NOTE.into());
    Ok(log)
}

#[cfg(target_os = "macos")]
const BUNDLE_BIN: &str = "Contents/MacOS/jpass";

/// Copy the binary into `/Applications/jpass.app`, or `~/Applications` when
/// that is not writable. Returns the bundle path.
#[cfg(target_os = "macos")]
fn install_bundle(exe: &Path, home: &Path, log: &mut Vec<String>) -> Result<PathBuf, String> {
    let system = PathBuf::from("/Applications/jpass.app");
    let user = home.join("Applications/jpass.app");
    if let Ok(done) = install_binary(exe, &system.join(BUNDLE_BIN)) {
        log.extend(done);
        return Ok(system);
    }
    log.extend(install_binary(exe, &user.join(BUNDLE_BIN))?);
    Ok(user)
}

#[cfg(target_os = "macos")]
const INFO_PLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
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

/// LaunchAgent: start hidden at login so Cmd+Space is always live. stdout and
/// stderr go to `log_file`, so a failed hotkey registration is not lost.
#[cfg(target_os = "macos")]
fn launch_agent(bin: &Path, log_file: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>com.jpass.app</string>\n\
         <key>ProgramArguments</key><array><string>{bin}</string><string>hidden</string></array>\n\
         <key>RunAtLoad</key><true/>\n\
         <key>KeepAlive</key><false/>\n\
         <key>ProcessType</key><string>Interactive</string>\n\
         <key>StandardOutPath</key><string>{log}</string>\n\
         <key>StandardErrorPath</key><string>{log}</string>\n\
         </dict></plist>\n",
        bin = bin.display(),
        log = log_file.display()
    )
}

/// Best effort: ad-hoc sign so Gatekeeper lets the copied binary run.
#[cfg(target_os = "macos")]
fn sign_bundle(app: &Path) {
    let _ = Command::new("codesign").args(["--force", "--deep", "-s", "-"]).arg(app).output();
    let _ = Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(app).output();
}

/// (Re)load the LaunchAgent, which also starts the background instance.
///
/// `bootstrap` alone is not enough: it fails when the label is already loaded,
/// so launchd would keep the definition from the last login (old arguments),
/// and since launchd pins the code hash of the binary it refuses to spawn a
/// reinstalled, re-signed binary ("spawn failed", OS_REASON_CODESIGNING).
/// Booting out first drops both. The failure of `bootout` is expected when
/// the agent is not loaded; a failed `bootstrap` is an install error.
#[cfg(target_os = "macos")]
fn reload_agent(agent_path: &Path) -> Result<String, String> {
    let uid = Command::new("id").arg("-u").output().map_err(|e| format!("cannot run id: {e}"))?;
    let domain = format!("gui/{}", String::from_utf8_lossy(&uid.stdout).trim());
    let _ = Command::new("launchctl").args(["bootout", &format!("{domain}/com.jpass.app")]).output();
    let out = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(agent_path)
        .output()
        .map_err(|e| format!("cannot run launchctl: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(format!("launchctl bootstrap {}: {}", agent_path.display(), msg.trim()));
    }
    Ok("loaded the LaunchAgent (jpass now runs in the background and starts at login)".into())
}

#[cfg(target_os = "macos")]
const HOTKEY_NOTE: &str = "hotkey: Cmd+Space while jpass runs in the background (disable Spotlight's \
                           Cmd+Space in System Settings → Keyboard → Keyboard Shortcuts → Spotlight)";

// ───────────────────────────── files ─────────────────────────────

fn own_binary() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))
}

/// Copy `src` to `dest` atomically (works even if `dest` is a running binary).
/// `None` when we already run from `dest`.
fn install_binary(src: &Path, dest: &Path) -> Result<Option<String>, String> {
    if same_file(src, dest) {
        return Ok(None);
    }
    let dir = dest.parent().ok_or("bad destination")?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = dir.join(".jpass.new");
    fs::copy(src, &tmp).map_err(|e| format!("cannot copy binary to {}: {e}", tmp.display()))?;
    make_executable(&tmp);
    fs::rename(&tmp, dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot install {}: {e}", dest.display())
    })?;
    Ok(Some(format!("installed {}", dest.display())))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
    }
}

fn write_file(path: &Path, contents: &str) -> Result<String, String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(format!("wrote {}", path.display()))
}
