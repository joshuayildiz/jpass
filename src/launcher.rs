//! Discover and launch desktop applications.
//!
//! Linux: parses `.desktop` files from XDG data directories.
//! macOS: scans `/Applications` and `~/Applications` for `.app` bundles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub exec: String,
    pub comment: String,
    pub icon_path: Option<String>,
    pub wm_class: Option<String>,
    /// `.desktop` file stem (Wayland app_id), e.g. `org.telegram.desktop`.
    pub desktop_id: String,
}

/// Every installed application, one per name.
pub fn scan() -> Vec<App> {
    let mut apps = platform::scan_apps();
    let mut seen = std::collections::HashSet::new();
    apps.retain(|a| seen.insert(a.name.to_lowercase()));
    apps
}

/// Bring the app's window to the front, or start it when it has none.
pub fn launch(app: &App) {
    platform::launch(app);
}

/// Bring an already open window to the front, by desktop id / window class.
#[cfg(not(target_os = "macos"))]
pub fn activate(desktop_id: &str, class: &str) -> bool {
    platform::activate(desktop_id, class)
}

// ───────────────────────────── icons ─────────────────────────────

/// A lazy texture cache keyed by icon file path.
pub struct IconCache(HashMap<String, Option<eframe::egui::TextureHandle>>);

impl IconCache {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn get(&mut self, ctx: &eframe::egui::Context, app: &App) -> Option<&eframe::egui::TextureHandle> {
        let path = app.icon_path.as_deref()?;
        self.0.entry(path.to_string()).or_insert_with(|| load_icon(ctx, path)).as_ref()
    }
}

/// Icons are rasterised at this size; drawn at `components::ICON` logical px.
const ICON_PX: u32 = 48;

/// PNG, JPEG, BMP and GIF are decoded natively; SVGs go through `rsvg-convert`.
fn load_icon(ctx: &eframe::egui::Context, path: &str) -> Option<eframe::egui::TextureHandle> {
    let p = Path::new(path);
    let rgba = match p.extension().and_then(|e| e.to_str()) {
        Some("svg" | "svgz") => load_svg(p)?,
        _ => image::open(p).ok()?.into_rgba8(),
    };
    let resized = image::imageops::resize(&rgba, ICON_PX, ICON_PX, image::imageops::FilterType::Lanczos3);
    let size = [resized.width() as usize, resized.height() as usize];
    let color = eframe::egui::ColorImage::from_rgba_unmultiplied(size, &resized.into_raw());
    Some(ctx.load_texture(path, color, eframe::egui::TextureOptions::LINEAR))
}

fn load_svg(path: &Path) -> Option<image::RgbaImage> {
    let px = ICON_PX.to_string();
    let out = Command::new("rsvg-convert").args(["-w", &px, "-h", &px]).arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    image::load_from_memory(&out.stdout).ok().map(|i| i.into_rgba8())
}

// ───────────────────────────── Linux ─────────────────────────────

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub fn scan_apps() -> Vec<App> {
        let data_dirs = xdg_data_dirs();
        let icon_dirs = icon_dirs(&data_dirs);
        let mut apps = Vec::new();
        for dir in &data_dirs {
            scan_dir(&dir.join("applications"), &icon_dirs, &mut apps);
        }
        apps
    }

    fn xdg_data_dirs() -> Vec<PathBuf> {
        let home = dirs::home_dir();
        let mut dirs = Vec::new();
        match std::env::var_os("XDG_DATA_HOME") {
            Some(d) => dirs.push(PathBuf::from(d)),
            None => dirs.extend(home.as_ref().map(|h| h.join(".local/share"))),
        }
        match std::env::var("XDG_DATA_DIRS") {
            Ok(d) => dirs.extend(d.split(':').filter(|s| !s.is_empty()).map(PathBuf::from)),
            Err(_) => dirs.extend(["/usr/local/share", "/usr/share"].map(PathBuf::from)),
        }
        dirs.extend(home.map(|h| h.join(".local/share/flatpak/exports/share")));
        dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
        dirs
    }

    fn scan_dir(dir: &Path, icon_dirs: &[PathBuf], apps: &mut Vec<App>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                scan_dir(&path, icon_dirs, apps);
            } else if path.extension().is_some_and(|e| e == "desktop") {
                apps.extend(parse_desktop(&path, icon_dirs));
            }
        }
    }

    fn parse_desktop(path: &Path, icon_dirs: &[PathBuf]) -> Option<App> {
        let text = std::fs::read_to_string(path).ok()?;
        let entry = desktop_entry(&text);
        let flag = |key: &str| entry.get(key).is_some_and(|v| v.eq_ignore_ascii_case("true"));
        if entry.get("Type").copied() != Some("Application") || flag("NoDisplay") || flag("Hidden") {
            return None;
        }
        Some(App {
            name: entry.get("Name")?.to_string(),
            exec: strip_field_codes(entry.get("Exec")?),
            comment: entry.get("Comment").unwrap_or(&"").to_string(),
            icon_path: entry.get("Icon").and_then(|icon| resolve_icon(icon, icon_dirs)),
            wm_class: entry.get("StartupWMClass").map(|c| c.to_string()),
            desktop_id: path.file_stem()?.to_string_lossy().into_owned(),
        })
    }

    /// The keys of the `[Desktop Entry]` group.
    fn desktop_entry(text: &str) -> HashMap<&str, &str> {
        let mut entry = HashMap::new();
        let mut in_group = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_group = line == "[Desktop Entry]";
            } else if in_group
                && let Some((key, value)) = line.split_once('=')
            {
                entry.insert(key.trim(), value.trim());
            }
        }
        entry
    }

    /// Drop `%f`-style field codes from an Exec line; `%%` is a literal `%`.
    fn strip_field_codes(exec: &str) -> String {
        let mut out = String::with_capacity(exec.len());
        let mut chars = exec.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
            } else if chars.next() == Some('%') {
                out.push('%');
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    // ── icons ──

    /// Every existing `apps` directory of the current theme and of hicolor,
    /// best first, then the pixmaps fallback. Computed once per scan.
    fn icon_dirs(data_dirs: &[PathBuf]) -> Vec<PathBuf> {
        let themes = [detect_icon_theme(), "hicolor".to_string()];
        let mut dirs: Vec<PathBuf> = themes
            .iter()
            .flat_map(|theme| data_dirs.iter().map(move |d| d.join("icons").join(theme)))
            .flat_map(|base| SIZES.iter().map(move |size| base.join(size).join("apps")))
            .filter(|d| d.is_dir())
            .collect();
        dirs.push(PathBuf::from("/usr/share/pixmaps"));
        dirs
    }

    /// Icon theme sizes, best first for drawing at ~24 logical px. Breeze
    /// uses bare numbers (`48/`), hicolor `WxH` (`48x48/`).
    const SIZES: [&str; 17] = [
        "48", "48x48", "scalable", "32", "32x32", "64", "64x64", "24", "24x24", "256x256", "128x128", "96x96",
        "72x72", "22", "22x22", "16", "16x16",
    ];
    const EXTS: [&str; 2] = ["svg", "png"];

    fn detect_icon_theme() -> String {
        crate::desktop::kdeglobals("Icons", "Theme")
            .or_else(|| crate::desktop::gsettings("org.gnome.desktop.interface", "icon-theme"))
            .unwrap_or_else(|| "hicolor".to_string())
    }

    fn resolve_icon(icon: &str, icon_dirs: &[PathBuf]) -> Option<String> {
        let candidates: Vec<PathBuf> = if icon.starts_with('/') {
            vec![PathBuf::from(icon)]
        } else {
            icon_dirs.iter().flat_map(|d| EXTS.iter().map(move |ext| d.join(format!("{icon}.{ext}")))).collect()
        };
        let found = candidates.into_iter().find(|p| p.exists())?;
        Some(found.to_string_lossy().into_owned())
    }

    // ── launch / focus ──

    pub fn launch(app: &App) {
        if focus_existing(app) {
            return;
        }
        spawn_detached(Command::new("sh").args(["-c", app.exec.as_str()]));
    }

    fn focus_existing(app: &App) -> bool {
        let class = app.wm_class.as_deref().unwrap_or_else(|| binary_name(&app.exec));
        activate(&app.desktop_id, class)
    }

    /// `/usr/bin/firefox --new-window` → `firefox`, the usual X11 window class.
    fn binary_name(exec: &str) -> &str {
        exec.split_whitespace().next().and_then(|b| b.rsplit('/').next()).unwrap_or("")
    }

    pub fn activate(desktop_id: &str, class: &str) -> bool {
        if class.is_empty() && desktop_id.is_empty() {
            return false;
        }
        focus_kwin(desktop_id, class) || (!class.is_empty() && focus_xdotool(class))
    }

    /// KDE: activate the window through a KWin script, which sees Wayland and
    /// X11 windows alike and is exempt from focus-stealing prevention.
    fn focus_kwin(desktop_id: &str, class: &str) -> bool {
        let file = std::env::temp_dir().join("jpass-focus.js");
        if std::fs::write(&file, focus_script(desktop_id, class)).is_err() {
            return false;
        }
        let found = run_kwin_script(&file);
        let _ = std::fs::remove_file(&file);
        found
    }

    /// Activates the first window whose desktop file or class matches, and
    /// reports "found" by unloading itself.
    fn focus_script(desktop_id: &str, class: &str) -> String {
        format!(
            "var id = \"{id}\", cls = \"{cls}\";\n\
             var w = workspace.windowList();\n\
             for (var i = 0; i < w.length; i++) {{\n\
               var c = w[i];\n\
               var dn = (c.desktopFileName || \"\").toString().toLowerCase().replace(/\\.desktop$/, \"\");\n\
               var rc = (c.resourceClass || \"\").toString().toLowerCase();\n\
               if ((id && dn === id) || (cls && rc === cls) || (id && rc === id)) {{\n\
                 workspace.activeWindow = c;\n\
                 callDBus(\"org.kde.KWin\", \"/Scripting\", \"org.kde.kwin.Scripting\", \"unloadScript\", \"{name}\");\n\
                 break;\n\
               }}\n\
             }}\n",
            id = js_str(desktop_id),
            cls = js_str(class),
            name = KWIN_SCRIPT,
        )
    }

    fn js_str(s: &str) -> String {
        s.to_lowercase().replace('\\', "\\\\").replace('"', "\\\"")
    }

    /// Load and start the script, then poll whether it unloaded itself. KWin
    /// reads the file in `start()`, asynchronously, so the file must still
    /// exist then and the outcome only shows up a moment later.
    fn run_kwin_script(file: &Path) -> bool {
        unload_kwin_script();
        let started = load_kwin_script(file) && kwin("start", &[]).is_some();
        let found = started && (0..20).any(|_| script_unloaded_itself());
        unload_kwin_script();
        found
    }

    const KWIN_SCRIPT: &str = "jpass_focus";

    /// False when KWin refused the script (it answers with id -1).
    fn load_kwin_script(file: &Path) -> bool {
        let id = kwin("loadScript", &["ss", &file.to_string_lossy(), KWIN_SCRIPT]);
        id.is_some_and(|id| !id.ends_with("-1"))
    }

    fn unload_kwin_script() {
        kwin("unloadScript", &["s", KWIN_SCRIPT]);
    }

    fn script_unloaded_itself() -> bool {
        std::thread::sleep(std::time::Duration::from_millis(25));
        kwin("isScriptLoaded", &["s", KWIN_SCRIPT]).as_deref() == Some("b false")
    }

    fn kwin(method: &str, args: &[&str]) -> Option<String> {
        crate::desktop::dbus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", method, args)
    }

    /// X11 (no KWin): activate the first window of this class.
    fn focus_xdotool(class: &str) -> bool {
        let Some(ids) = xdotool(&["search", "--class", class]) else { return false };
        let Some(first) = ids.lines().next() else { return false };
        xdotool(&["windowactivate", first]).is_some()
    }

    fn xdotool(args: &[&str]) -> Option<String> {
        let out = Command::new("xdotool").args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The live tests share one KWin script slot, so they must not overlap.
        static LIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());

        /// Manual check on a live desktop: with Firefox open, it must be
        /// activated rather than started again.
        /// `cargo test focus_running_firefox -- --ignored`
        #[test]
        #[ignore]
        fn focus_running_firefox() {
            let _live = LIVE.lock().unwrap();
            let app = scan().into_iter().find(|a| a.desktop_id == "firefox").expect("firefox.desktop");
            assert!(focus_existing(&app), "no Firefox window was found/activated");
        }

        /// Live desktop: an app without a window must report "not found" so it gets launched.
        #[test]
        #[ignore]
        fn no_window_means_not_found() {
            let _live = LIVE.lock().unwrap();
            let app = App {
                name: "nope".into(),
                exec: "jpass-no-such-binary".into(),
                comment: String::new(),
                icon_path: None,
                wm_class: None,
                desktop_id: "jpass-no-such-app".into(),
            };
            assert!(!focus_existing(&app));
        }

        #[test]
        fn strip_field_codes_works() {
            assert_eq!(strip_field_codes("firefox %u"), "firefox");
            assert_eq!(strip_field_codes("app %F --flag"), "app --flag");
            assert_eq!(strip_field_codes("100%%"), "100%");
            assert_eq!(strip_field_codes("plain"), "plain");
        }

        #[test]
        fn desktop_entry_reads_only_the_main_group() {
            let text = "[Desktop Entry]\nName=Foo\nExec=foo %u\n[Desktop Action new]\nName=New\n";
            let e = desktop_entry(text);
            assert_eq!(e.get("Name"), Some(&"Foo"));
            assert_eq!(e.get("Exec"), Some(&"foo %u"));
        }
    }
}

// ───────────────────────────── macOS ─────────────────────────────

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub fn scan_apps() -> Vec<App> {
        let mut apps = Vec::new();
        scan_macos_dir(Path::new("/Applications"), &mut apps);
        if let Some(h) = dirs::home_dir() {
            scan_macos_dir(&h.join("Applications"), &mut apps);
        }
        apps
    }

    fn scan_macos_dir(dir: &Path, apps: &mut Vec<App>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().is_some_and(|e| e == "app") {
                let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                apps.push(App {
                    exec: path.to_string_lossy().to_string(),
                    desktop_id: name.clone(),
                    name,
                    comment: String::new(),
                    icon_path: None,
                    wm_class: None,
                });
            }
        }
    }

    pub fn launch(app: &App) {
        spawn_detached(Command::new("open").args(["-a", app.exec.as_str()]));
    }
}

/// Start a program with no terminal attached and without waiting for it.
fn spawn_detached(cmd: &mut Command) {
    let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_some_apps() {
        let apps = scan();
        assert!(!apps.is_empty(), "should find at least one application");
    }

    #[test]
    fn no_duplicate_names() {
        let apps = scan();
        let mut seen = std::collections::HashSet::new();
        for app in &apps {
            assert!(seen.insert(app.name.to_lowercase()), "duplicate: {}", app.name);
        }
    }

    #[test]
    fn some_icons_resolve() {
        let apps = scan();
        let with_icon = apps.iter().filter(|a| a.icon_path.is_some()).count();
        assert!(with_icon > 0, "should resolve at least one icon path");
    }
}
