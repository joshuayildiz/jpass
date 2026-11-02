mod install;
mod pwgen;
mod vault;

use age::secrecy::SecretString;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontData, FontFamily, FontId, Key, Layout,
    Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2,
};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};
use vault::{Entry, Stamp, Vault};

const CLIPBOARD_CLEAR: Duration = Duration::from_secs(20);
/// After the window has been hidden this long, the vault is locked again.
const RELOCK_AFTER: Duration = Duration::from_secs(5 * 60);

const CMD_NONE: u8 = 0;
const CMD_TOGGLE: u8 = 1;
const CMD_SHOW: u8 = 2;

// Layout (logical px). The window is a bit larger than the visible panel so
// the drop shadow has room.
const MARGIN: f32 = 14.0;
const PANEL_W: f32 = 640.0;
const WIN_W: f32 = PANEL_W + MARGIN * 2.0;
const BAR_H: f32 = 64.0;
const ROW_H: f32 = 54.0;
const MAX_ROWS: usize = 7;
const FOOT_H: f32 = 30.0;
/// Left/right margin of everything inside the panel (icon, hints, links, forms).
const GUTTER: f32 = 28.0;
/// Text column: where the input text and the row text start, so they share one vertical axis.
const TEXT_X: f32 = 60.0;
/// A revealed row gets a third line.
const REVEAL_EXTRA: f32 = 18.0;
/// Height reserved for the error banner.
const ERR_H: f32 = 48.0;
/// Breathing room under the last element of the lock screen and the forms.
const FORM_PAD: f32 = 22.0;

// What is on screen; used to match a measured height to the screen it was measured on.
const SCREEN_WAITING: u8 = 0;
const SCREEN_LOCKED: u8 = 1;
const SCREEN_CREATE: u8 = 2;
const SCREEN_EDIT: u8 = 3;
const SCREEN_DELETE: u8 = 4;
const SCREEN_LIST: u8 = 5;

// ───────────────────────────── theme ─────────────────────────────

#[derive(Clone, Copy)]
struct Pal {
    dark: bool,
    bg: Color32,
    border: Color32,
    text: Color32,
    weak: Color32,
    accent: Color32,
    sel: Color32,
    field: Color32,
    danger: Color32,
    ok: Color32,
    /// Hard, un-blurred offset shadow (Elm's "terminal" box).
    shadow: Color32,
}

impl Pal {
    fn new(dark: bool) -> Pal {
        // Light: elm-lang.org blue on white, ink from acadia.engineering.
        // Dark: Acadia's cool gray scale.
        if dark {
            Pal {
                dark,
                bg: Color32::from_rgb(16, 24, 40),
                border: Color32::from_rgb(52, 64, 85),
                text: Color32::from_rgb(243, 244, 247),
                weak: Color32::from_rgb(152, 162, 180),
                accent: Color32::from_rgb(83, 177, 253),
                sel: Color32::from_rgb(29, 41, 57),
                field: Color32::from_rgb(16, 24, 40),
                danger: Color32::from_rgb(247, 92, 134),
                ok: Color32::from_rgb(50, 213, 131),
                shadow: Color32::from_black_alpha(200),
            }
        } else {
            Pal {
                dark,
                bg: Color32::WHITE,
                border: Color32::from_rgb(208, 213, 221),
                text: Color32::from_rgb(17, 22, 17),
                weak: Color32::from_rgb(102, 112, 133),
                accent: Color32::from_rgb(18, 147, 216),
                sel: Color32::from_rgb(243, 244, 247),
                field: Color32::WHITE,
                danger: Color32::from_rgb(227, 28, 84),
                ok: Color32::from_rgb(7, 148, 85),
                shadow: Color32::from_rgb(215, 218, 224),
            }
        }
    }
}

/// Ask the desktop whether it is in dark mode. Defaults to dark.
fn detect_dark() -> bool {
    use std::process::Command;
    let out = |cmd: &str, args: &[&str]| -> Option<String> {
        let o = Command::new(cmd).args(args).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).to_lowercase())
    };
    #[cfg(target_os = "macos")]
    {
        return out("defaults", &["read", "-g", "AppleInterfaceStyle"])
            .is_some_and(|s| s.contains("dark"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        // KDE: luminance of the window background colour in kdeglobals.
        if let Some(cfg) = dirs::config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("kdeglobals")).ok())
        {
            let mut in_sec = false;
            for line in cfg.lines() {
                let l = line.trim();
                if l.starts_with('[') {
                    in_sec = l == "[Colors:Window]";
                } else if in_sec {
                    if let Some(v) = l.strip_prefix("BackgroundNormal=") {
                        let c: Vec<f32> =
                            v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        if c.len() >= 3 {
                            return 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2] < 128.0;
                        }
                    }
                }
            }
        }
        // GNOME & friends.
        if let Some(s) = out("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"]) {
            if s.contains("dark") {
                return true;
            }
            if s.contains("light") || s.contains("default") {
                return false;
            }
        }
        true
    }
}

// ───────────────────────────── single instance / IPC ─────────────────────────────

fn socket_path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(d).join("jpass.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("jpass-{user}.sock"))
}

fn send_command(cmd: &str) -> bool {
    match UnixStream::connect(socket_path()) {
        Ok(mut s) => s.write_all(format!("{cmd}\n").as_bytes()).is_ok(),
        Err(_) => false,
    }
}

enum Claim {
    /// We own the socket: this process is the instance.
    /// `None` when the socket could not be created: run anyway, without IPC.
    Server(Option<UnixListener>),
    /// Another jpass is already running.
    AlreadyRunning,
}

/// Atomically become the single running instance (bind is the lock).
fn claim_socket() -> Claim {
    let path = socket_path();
    for _ in 0..2 {
        match UnixListener::bind(&path) {
            Ok(l) => return Claim::Server(Some(l)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if UnixStream::connect(&path).is_ok() {
                    return Claim::AlreadyRunning;
                }
                let _ = std::fs::remove_file(&path); // stale socket from a crash
            }
            Err(e) => {
                eprintln!("jpass: cannot create {} ({e}); running without single-instance support", path.display());
                return Claim::Server(None);
            }
        }
    }
    Claim::AlreadyRunning
}

fn serve(listener: Option<UnixListener>, cmd: Arc<AtomicU8>, ctx: egui::Context) {
    let Some(listener) = listener else { return };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let _ = BufReader::new(stream).read_line(&mut line);
            match line.trim() {
                "toggle" => cmd.store(CMD_TOGGLE, Ordering::SeqCst),
                "show" => cmd.store(CMD_SHOW, Ordering::SeqCst),
                _ => continue,
            }
            ctx.request_repaint();
        }
    });
}

// ───────────────────────────── app ─────────────────────────────

enum JobResult {
    Loaded { vault: Vault, stamp: Stamp, pass: SecretString },
    Saved { vault: Vault, stamp: Stamp },
    Failed(String),
}

#[derive(Default)]
struct EditState {
    /// Index into `vault.entries`; `None` = new entry.
    index: Option<usize>,
    account: String,
    user: String,
    password: String,
    show_password: bool,
    focus_first: bool,
}

impl EditState {
    /// A blank entry that starts with a freshly generated strong password.
    fn new_entry() -> Self {
        EditState { focus_first: true, password: pwgen::generate(), show_password: true, ..Default::default() }
    }
}

struct App {
    path: PathBuf,
    file_exists: bool,
    /// The folder holding the vault is available (pCloud is mounted).
    drive_ready: bool,
    pal: Pal,

    // session
    pass: Option<SecretString>,
    vault: Option<Vault>,
    stamp: Option<Stamp>,

    // ui state
    pass_input: String,
    pass_confirm: String,
    error: String,
    status: Option<(String, Instant)>,
    search: String,
    selected: usize,
    revealed: Option<usize>,
    edit: Option<EditState>,
    confirm_delete: Option<usize>,
    busy: Option<&'static str>,
    need_reload: bool,
    scroll_to_sel: bool,

    // window state
    visible: bool,
    hidden_at: Option<Instant>,
    shown_at: Instant,
    seen_focus: bool,
    /// Keep asking the window manager for focus until this instant (or until granted).
    focus_deadline: Option<Instant>,
    focus_input: bool,
    need_center: bool,
    last_height: f32,
    /// (screen, content height) measured on the last frame of a non-list screen.
    measured: Option<(u8, f32)>,

    // plumbing
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    cmd: Arc<AtomicU8>,
    clipboard: Arc<Mutex<Option<arboard::Clipboard>>>,
    _hotkey: Option<GlobalHotKeyManager>,
}

const PLEX_SANS: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");
const PLEX_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf");
const PLEX_MONO: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

/// IBM Plex, as used on elm-lang.org and acadia.engineering.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [("plex", PLEX_SANS), ("plex-semibold", PLEX_SEMIBOLD), ("plex-mono", PLEX_MONO)] {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "plex".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "plex-mono".into());
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), vec!["plex-semibold".into(), "plex".into()]);
    ctx.set_fonts(fonts);
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

fn apply_theme(ctx: &egui::Context, p: &Pal) {
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.override_text_color = Some(p.text);
    v.selection.bg_fill = p.accent.gamma_multiply(0.3);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.extreme_bg_color = p.field;
    v.window_fill = p.bg;
    v.panel_fill = Color32::TRANSPARENT;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_fill = p.field;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.hovered.weak_bg_fill = p.sel;
    v.widgets.active.bg_stroke = Stroke::new(2.0, p.accent);
    v.widgets.active.weak_bg_fill = p.sel;
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.noninteractive,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::ZERO;
    }
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(10.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
    });
}

/// Plain text link in the accent colour; underlined on hover (elm-lang.org).
fn link(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(13.0), color);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(0.0, 6.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let pos = rect.left_center() - vec2(0.0, galley.size().y / 2.0);
    let bottom = pos.y + galley.size().y;
    ui.painter().galley(pos, galley, color);
    if resp.hovered() {
        ui.painter().hline(rect.x_range(), bottom, Stroke::new(1.0, color));
    }
    resp
}

/// Elm-style button: square, 2px outline, inverts on hover.
fn flat_button(ui: &mut egui::Ui, label: &str, color: Color32, bg: Color32, enabled: bool) -> egui::Response {
    let font = FontId::proportional(14.5);
    let size = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), color).size() + vec2(32.0, 14.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hot = enabled && resp.hovered();
    let line = if enabled { color } else { color.gamma_multiply(0.4) };
    let (fill, fg) = if hot { (line, bg) } else { (Color32::TRANSPARENT, line) };
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().rect_stroke(rect, 0.0, Stroke::new(2.0, line), StrokeKind::Inside);
    let g = ui.painter().layout_no_wrap(label.to_owned(), font, fg);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, start_hidden: bool, listener: Option<UnixListener>) -> Self {
        let cmd = Arc::new(AtomicU8::new(CMD_NONE));
        serve(listener, cmd.clone(), cc.egui_ctx.clone());

        // Built-in global hotkey (macOS + X11). Everywhere else, bind
        // `jpass --toggle` to a key in the desktop's shortcut settings.
        let mut hk_mgr = None;
        if let Ok(mgr) = GlobalHotKeyManager::new() {
            // The Cmd key: `Super` on macOS. On this Linux keyboard layout the
            // physical Cmd key is reported as Alt (Super sits on Option).
            #[cfg(target_os = "macos")]
            let mods = Modifiers::META;
            #[cfg(not(target_os = "macos"))]
            let mods = Modifiers::ALT;
            let hk = HotKey::new(Some(mods), Code::Space);
            if mgr.register(hk).is_ok() {
                let c = cmd.clone();
                let ctx = cc.egui_ctx.clone();
                let id = hk.id();
                std::thread::spawn(move || {
                    while let Ok(ev) = GlobalHotKeyEvent::receiver().recv() {
                        if ev.id == id && ev.state == HotKeyState::Pressed {
                            c.store(CMD_TOGGLE, Ordering::SeqCst);
                            ctx.request_repaint();
                        }
                    }
                });
                hk_mgr = Some(mgr);
            }
        }

        let pal = Pal::new(detect_dark());
        install_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx, &pal);

        let (tx, rx) = channel();
        let path = vault::default_path();
        #[allow(unused_mut)]
        let mut app = App {
            file_exists: path.exists(),
            drive_ready: vault::drive_ready(path.parent().unwrap_or(std::path::Path::new("."))),
            path,
            pal,
            pass: None,
            vault: None,
            stamp: None,
            pass_input: String::new(),
            pass_confirm: String::new(),
            error: String::new(),
            status: None,
            search: String::new(),
            selected: 0,
            revealed: None,
            edit: None,
            confirm_delete: None,
            busy: None,
            need_reload: false,
            scroll_to_sel: false,
            visible: !start_hidden,
            hidden_at: start_hidden.then(Instant::now),
            shown_at: Instant::now(),
            seen_focus: false,
            focus_deadline: None,
            focus_input: true,
            need_center: true,
            last_height: 0.0,
            measured: None,
            tx,
            rx,
            cmd,
            clipboard: Arc::new(Mutex::new(arboard::Clipboard::new().ok())),
            _hotkey: hk_mgr,
        };
        #[cfg(debug_assertions)]
        if let Ok(q) = std::env::var("JPASS_DEV_SEARCH") {
            app.search = q;
        }
        // Dev builds only: auto-unlock for UI testing. Compiled out of release builds.
        #[cfg(debug_assertions)]
        if let Ok(p) = std::env::var("JPASS_DEV_PASS") {
            app.start_load(SecretString::from(p), "Decrypting…");
        }
        app
    }

    // ── session control ──

    fn lock(&mut self) {
        self.pass = None;
        self.vault = None;
        self.stamp = None;
        self.pass_input.clear();
        self.pass_confirm.clear();
        self.search.clear();
        self.edit = None;
        self.confirm_delete = None;
        self.revealed = None;
        self.need_reload = false;
        self.focus_input = true;
        self.refresh_file_state();
    }

    /// Re-check the drive and the file; both can change after startup (pCloud mounts late).
    fn refresh_file_state(&mut self) {
        let dir = self.path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
        self.drive_ready = vault::drive_ready(&dir);
        self.file_exists = self.drive_ready && self.path.exists();
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        self.search.clear();
        self.edit = None;
        self.confirm_delete = None;
        self.revealed = None;
        self.selected = 0;
        self.error.clear();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        // Also hide the application itself: AppKit then activates the app that
        // was in front before jpass, so the keyboard goes back where it was.
        #[cfg(target_os = "macos")]
        macos::hide_app();
    }

    fn show(&mut self, ctx: &egui::Context) {
        if self.hidden_at.is_some_and(|t| t.elapsed() > RELOCK_AFTER) {
            self.lock();
        }
        self.refresh_file_state();
        // follow light/dark changes
        self.pal = Pal::new(detect_dark());
        apply_theme(ctx, &self.pal);

        self.visible = true;
        self.shown_at = Instant::now();
        self.seen_focus = false;
        self.focus_input = true;
        self.need_center = true;
        // Undo `hide_app`: a hidden application keeps its windows off screen
        // even after they are ordered front, so unhide before showing.
        #[cfg(target_os = "macos")]
        macos::unhide_app();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        // `Focus` is ignored while the window is still being mapped, so it is
        // (re)sent from `ui()` on the following frames until focus is granted.
        self.focus_deadline = Some(Instant::now() + Duration::from_millis(1500));
        // Pick up changes pCloud synced from another machine.
        if self.vault.is_some() && self.busy.is_none() && !self.same_on_disk() {
            self.start_load(self.pass.clone().unwrap(), "Reloading…");
        }
    }

    fn same_on_disk(&self) -> bool {
        match (&self.stamp, vault::stamp(&self.path)) {
            (Some(a), Some(b)) => a.same_file(&b),
            _ => false,
        }
    }

    fn handle_command(&mut self, ctx: &egui::Context) {
        match self.cmd.swap(CMD_NONE, Ordering::SeqCst) {
            CMD_TOGGLE => {
                if self.visible {
                    self.hide(ctx)
                } else {
                    self.show(ctx)
                }
            }
            CMD_SHOW => self.show(ctx),
            _ => {}
        }
    }

    // ── jobs ──

    fn start_load(&mut self, pass: SecretString, msg: &'static str) {
        self.busy = Some(msg);
        self.error.clear();
        let (tx, path) = (self.tx.clone(), self.path.clone());
        std::thread::spawn(move || {
            let r = match vault::load(&path, &pass) {
                Ok((vault, stamp)) => JobResult::Loaded { vault, stamp, pass },
                Err(e) => JobResult::Failed(e),
            };
            let _ = tx.send(r);
        });
    }

    fn start_create(&mut self, pass: SecretString) {
        self.busy = Some("Creating…");
        self.error.clear();
        let (tx, path) = (self.tx.clone(), self.path.clone());
        std::thread::spawn(move || {
            let v = Vault {
                header: Some(vec!["Account".into(), "Identifier".into(), "Password".into()]),
                entries: vec![],
            };
            let r = match vault::save(&path, &v, &pass, None) {
                Ok(_) => match vault::load(&path, &pass) {
                    Ok((vault, stamp)) => JobResult::Loaded { vault, stamp, pass },
                    Err(e) => JobResult::Failed(e),
                },
                Err(e) => JobResult::Failed(e),
            };
            let _ = tx.send(r);
        });
    }

    /// Persist `new_vault`; it becomes the live vault only if the save succeeds.
    fn start_save(&mut self, new_vault: Vault) {
        let Some(pass) = self.pass.clone() else { return };
        self.busy = Some("Saving…");
        self.error.clear();
        let (tx, path, stamp) = (self.tx.clone(), self.path.clone(), self.stamp.clone());
        std::thread::spawn(move || {
            let r = match vault::save(&path, &new_vault, &pass, stamp.as_ref()) {
                Ok(stamp) => JobResult::Saved { vault: new_vault, stamp },
                Err(e) => JobResult::Failed(e),
            };
            let _ = tx.send(r);
        });
    }

    fn poll_jobs(&mut self) {
        while let Ok(r) = self.rx.try_recv() {
            self.busy = None;
            match r {
                JobResult::Loaded { vault, stamp, pass } => {
                    self.vault = Some(vault);
                    self.stamp = Some(stamp);
                    self.pass = Some(pass);
                    self.pass_input.clear();
                    self.pass_confirm.clear();
                    self.need_reload = false;
                    self.file_exists = true;
                    self.focus_input = true;
                    // Dev builds only: jump straight to a form for visual checks.
                    #[cfg(debug_assertions)]
                    match std::env::var("JPASS_DEV_VIEW").as_deref() {
                        Ok("edit") => self.begin_edit(0),
                        Ok("new") => self.edit = Some(EditState::new_entry()),
                        Ok("delete") => self.confirm_delete = Some(0),
                        Ok("reveal") => {
                            self.revealed = Some(0);
                            self.toast("Copied password · clears in 20s");
                        }
                        Ok("error") => self.error = "Wrong passphrase (or file is corrupted)".into(),
                        _ => {}
                    }
                }
                JobResult::Saved { vault, stamp } => {
                    self.vault = Some(vault);
                    self.stamp = Some(stamp);
                    self.toast("Saved");
                }
                JobResult::Failed(e) => {
                    if self.vault.is_some() && e.contains("changed on disk") {
                        self.need_reload = true;
                    }
                    self.error = e;
                }
            }
        }
    }

    // ── helpers ──

    fn toast(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn copy(&mut self, what: &str, text: &str) {
        let ok = {
            let mut guard = self.clipboard.lock().unwrap();
            guard.as_mut().is_some_and(|cb| cb.set_text(text.to_owned()).is_ok())
        };
        if !ok {
            self.error = "Could not access the clipboard".into();
            return;
        }
        self.toast(format!("Copied {what} · clears in {}s", CLIPBOARD_CLEAR.as_secs()));
        let (cb, expected) = (self.clipboard.clone(), text.to_owned());
        std::thread::spawn(move || {
            std::thread::sleep(CLIPBOARD_CLEAR);
            if let Some(cb) = cb.lock().unwrap().as_mut() {
                if cb.get_text().is_ok_and(|t| t == expected) {
                    let _ = cb.clear();
                }
            }
        });
    }

    fn filtered(&self) -> Vec<usize> {
        let Some(v) = &self.vault else { return vec![] };
        let words: Vec<String> = self.search.split_whitespace().map(str::to_lowercase).collect();
        let mut idx: Vec<usize> = (0..v.entries.len())
            .filter(|&i| {
                let e = &v.entries[i];
                let hay = format!("{} {}", e.account(), e.user()).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect();
        idx.sort_by_key(|&i| v.entries[i].account().to_lowercase());
        idx
    }

    fn row_h(&self, i: usize) -> f32 {
        ROW_H + if self.revealed == Some(i) { REVEAL_EXTRA } else { 0.0 }
    }

    /// Exact height of the rows currently shown in the list.
    fn rows_h(&self, list: &[usize]) -> f32 {
        list.iter().take(MAX_ROWS).map(|&i| self.row_h(i)).sum::<f32>().max(ROW_H)
    }

    fn screen_key(&self) -> u8 {
        if self.vault.is_none() {
            if !self.drive_ready {
                SCREEN_WAITING
            } else if self.file_exists {
                SCREEN_LOCKED
            } else {
                SCREEN_CREATE
            }
        } else if self.edit.is_some() {
            SCREEN_EDIT
        } else if self.confirm_delete.is_some() {
            SCREEN_DELETE
        } else {
            SCREEN_LIST
        }
    }

    /// Height of the visible panel. The list is computed exactly; every other
    /// screen uses the height measured on the previous frame (so nothing can
    /// be clipped or leave dead space), with a rough estimate for the first frame.
    fn desired_height(&self, list: &[usize]) -> f32 {
        let err = if self.error.is_empty() { 0.0 } else { ERR_H };
        let key = self.screen_key();
        if key == SCREEN_LIST {
            return err + BAR_H + 1.0 + self.rows_h(list) + FOOT_H;
        }
        match self.measured {
            Some((k, h)) if k == key => h + FORM_PAD,
            _ => {
                err + FORM_PAD
                    + match key {
                        SCREEN_WAITING => BAR_H + 50.0,
                        SCREEN_LOCKED => BAR_H + 60.0,
                        SCREEN_CREATE => BAR_H + 190.0,
                        SCREEN_EDIT => 330.0,
                        _ => 150.0,
                    }
            }
        }
    }

    // ── drawing ──

    /// The 64px input bar: glyph on the left gutter, text on the shared text column.
    /// Leaves the layout cursor exactly at the bottom of the bar.
    fn search_bar(&mut self, ui: &mut egui::Ui, hint: &str, locked: bool) -> egui::Response {
        let p = self.pal;
        let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
        // Glyph: ink left edge on the gutter, optically centred on the text.
        let cy = rect.center().y + 0.5;
        let st = Stroke::new(1.6, p.text);
        if locked {
            let body = Rect::from_min_size(pos2(rect.left() + GUTTER, cy - 3.0), vec2(15.0, 11.0));
            ui.painter().rect_stroke(body, 0.0, st, StrokeKind::Inside);
            ui.painter().circle_stroke(pos2(body.center().x, body.top()), 4.5, st);
        } else {
            let c = pos2(rect.left() + GUTTER + 7.3, cy - 1.5);
            ui.painter().circle_stroke(c, 6.5, st);
            ui.painter().line_segment([c + vec2(4.8, 4.8), c + vec2(9.5, 9.5)], st);
        }
        // Measured: with this box the x-height band of the 20px text and the glyph share one centre.
        let field = Rect::from_min_max(
            pos2(rect.left() + TEXT_X, rect.top() + 8.5),
            pos2(rect.right() - GUTTER, rect.bottom() - 15.5),
        );
        let text = if locked { &mut self.pass_input } else { &mut self.search };
        let mut r = None;
        ui.scope_builder(UiBuilder::new().max_rect(field).layout(Layout::left_to_right(Align::Center)), |ui| {
            r = Some(ui.add(
                egui::TextEdit::singleline(text)
                    .font(FontId::proportional(20.0))
                    .frame(egui::Frame::NONE)
                    .password(locked)
                    .hint_text(RichText::new(hint).font(FontId::proportional(20.0)).color(p.weak))
                    .desired_width(field.width())
                    .margin(vec2(0.0, 4.0)),
            ));
        });
        // `scope_builder` would otherwise leave the cursor inside the bar.
        ui.advance_cursor_after_rect(rect);
        ui.spacing_mut().item_spacing.y = gap;
        let r = r.unwrap();
        if self.focus_input {
            r.request_focus();
            self.focus_input = false;
        }
        r
    }

    /// 1px rule directly under the bar.
    fn separator(&self, ui: &mut egui::Ui) {
        let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, self.pal.border));
        ui.spacing_mut().item_spacing.y = gap;
    }

    /// A small grey line of text on the gutter.
    fn note(&self, ui: &mut egui::Ui, text: impl Into<String>) {
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.label(RichText::new(text.into()).color(self.pal.weak).size(13.0));
        });
    }

    /// Lowercase monospace key hints, same style as the list footer.
    fn hints(&self, ui: &mut egui::Ui, text: &str) {
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.label(RichText::new(text).font(FontId::monospace(11.0)).color(self.pal.weak));
        });
    }

    fn ui_locked(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        if !self.drive_ready {
            // pCloud is not mounted yet: never offer to create a vault in the bare mount point.
            ui.add_enabled_ui(false, |ui| {
                self.search_bar(ui, "Waiting for pCloud…", true);
            });
            self.focus_input = true;
            self.separator(ui);
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(GUTTER);
                ui.spinner();
                let dir = self.path.parent().map(|d| d.display().to_string()).unwrap_or_default();
                let dir = match dirs::home_dir().and_then(|h| h.to_str().map(str::to_owned)) {
                    Some(h) => dir.replacen(&h, "~", 1),
                    None => dir,
                };
                ui.label(RichText::new(format!("Waiting for {dir} to be mounted…")).color(p.weak).size(13.0));
            });
            return;
        }
        let creating = !self.file_exists;
        let busy = self.busy.is_some();

        let r = ui
            .add_enabled_ui(!busy, |ui| {
                self.search_bar(
                    ui,
                    if creating { "Choose a master passphrase" } else { "Master passphrase" },
                    true,
                )
            })
            .inner;
        let mut submit = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));

        self.separator(ui);
        ui.add_space(14.0);
        if creating {
            self.note(ui, "No password file yet. A new encrypted one will be created at");
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(GUTTER);
                let path = self.path.display().to_string();
                let path = match dirs::home_dir().and_then(|h| h.to_str().map(str::to_owned)) {
                    Some(h) => path.replacen(&h, "~", 1),
                    None => path,
                };
                ui.label(RichText::new(path).font(FontId::monospace(12.5)));
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(GUTTER);
                let r2 = ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(&mut self.pass_confirm)
                        .password(true)
                        .hint_text("Repeat passphrase")
                        .margin(vec2(8.0, 6.0))
                        .desired_width(PANEL_W - 2.0 * GUTTER),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    r2.request_focus();
                    submit = false;
                } else if r2.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
            });
            ui.add_space(14.0);
        }

        if let Some(msg) = self.busy {
            ui.horizontal(|ui| {
                ui.add_space(GUTTER);
                ui.spinner();
                ui.label(RichText::new(msg).color(p.weak).size(13.0));
            });
        } else {
            self.hints(ui, if creating { "enter create   esc close" } else { "enter unlock   esc close" });
        }

        if submit && !self.pass_input.is_empty() {
            if creating && self.pass_input != self.pass_confirm {
                self.error = "Passphrases do not match".into();
            } else {
                let pw = SecretString::from(std::mem::take(&mut self.pass_input));
                self.pass_confirm.clear();
                if creating {
                    self.start_create(pw);
                } else {
                    self.start_load(pw, "Decrypting…");
                }
            }
        }
    }

    fn ui_list(&mut self, ui: &mut egui::Ui, list: &[usize]) {
        let p = self.pal;
        let ctx = ui.ctx().clone();
        let busy = self.busy.is_some();

        let r = self.search_bar(ui, "Search accounts", false);
        if r.changed() {
            self.selected = 0;
            self.revealed = None;
        }
        self.separator(ui);

        if self.selected >= list.len() {
            self.selected = list.len().saturating_sub(1);
        }

        // keyboard
        if !busy {
            let (down, up, enter, ctrl_u, ctrl_n, ctrl_l, ctrl_r, ctrl_e, ctrl_d) = ctx.input(|i| {
                let c = i.modifiers.command;
                (
                    i.key_pressed(Key::ArrowDown),
                    i.key_pressed(Key::ArrowUp),
                    i.key_pressed(Key::Enter),
                    c && i.key_pressed(Key::U),
                    c && i.key_pressed(Key::N),
                    c && i.key_pressed(Key::L),
                    c && i.key_pressed(Key::R),
                    c && i.key_pressed(Key::E),
                    c && i.key_pressed(Key::D),
                )
            });
            if down && self.selected + 1 < list.len() {
                self.selected += 1;
                self.scroll_to_sel = true;
            }
            if up && self.selected > 0 {
                self.selected -= 1;
                self.scroll_to_sel = true;
            }
            if let Some(&i) = list.get(self.selected) {
                let e = self.vault.as_ref().unwrap().entries[i].clone();
                if enter {
                    self.copy("password", e.password());
                    self.hide(&ctx);
                    return;
                }
                if ctrl_u {
                    self.copy("identifier", e.user());
                }
                if ctrl_r {
                    self.revealed = if self.revealed == Some(i) { None } else { Some(i) };
                }
                if ctrl_e {
                    self.begin_edit(i);
                }
                if ctrl_d {
                    self.confirm_delete = Some(i);
                }
            }
            if ctrl_n {
                self.edit = Some(EditState::new_entry());
            }
            if ctrl_l {
                self.lock();
                return;
            }
        }

        // rows
        let mut action: Option<(usize, &str)> = None;
        egui::ScrollArea::vertical()
            .max_height(self.rows_h(list))
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if list.is_empty() {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        if self.vault.as_ref().is_some_and(|v| v.entries.is_empty()) {
                            "No entries yet — ctrl+n to add one"
                        } else {
                            "No matching entries"
                        },
                        FontId::proportional(14.0),
                        p.weak,
                    );
                }
                for (row, &i) in list.iter().enumerate() {
                    let e = self.vault.as_ref().unwrap().entries[i].clone();
                    let sel = row == self.selected;
                    let revealed = self.revealed == Some(i);
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), self.row_h(i)), Sense::click());
                    if sel {
                        ui.painter().rect_filled(rect, 0.0, p.sel);
                        // 3px accent rule on the left edge
                        ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, p.accent);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, p.sel.gamma_multiply(0.55));
                    }
                    if sel && self.scroll_to_sel {
                        resp.scroll_to_me(None);
                        self.scroll_to_sel = false;
                    }
                    // Text on the shared column (same x as the search text). Lines are 18px apart.
                    let x = rect.left() + TEXT_X;
                    let cy = rect.center().y;
                    let (ya, yb, yc) = if revealed { (cy - 19.0, cy - 1.0, cy + 17.0) } else { (cy - 9.5, cy + 8.5, 0.0) };
                    ui.painter().text(pos2(x, ya), Align2::LEFT_CENTER, e.account(), semibold(15.5), p.text);
                    ui.painter().text(pos2(x, yb), Align2::LEFT_CENTER, e.user(), FontId::monospace(12.0), p.weak);
                    if revealed {
                        ui.painter().text(pos2(x, yc), Align2::LEFT_CENTER, e.password(), FontId::monospace(12.0), p.accent);
                    }
                    if resp.double_clicked() {
                        self.selected = row;
                        action = Some((i, "pass"));
                    } else if resp.clicked() {
                        self.selected = row;
                    }
                    // quiet text links on the active row, centred on the first 54px of the row
                    if sel || resp.hovered() {
                        let br = Rect::from_min_max(
                            pos2(rect.right() - 330.0, rect.top()),
                            pos2(rect.right() - GUTTER, rect.top() + ROW_H),
                        );
                        ui.scope_builder(
                            UiBuilder::new().max_rect(br).layout(Layout::right_to_left(Align::Center)),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 16.0;
                                for (label, what, color) in [
                                    ("delete", "delete", p.danger),
                                    ("edit", "edit", p.accent),
                                    ("show", "reveal", p.accent),
                                    ("user", "user", p.accent),
                                    ("copy", "pass", p.accent),
                                ] {
                                    if link(ui, label, color).clicked() {
                                        action = Some((i, what));
                                    }
                                }
                            },
                        );
                        ui.advance_cursor_after_rect(rect);
                    }
                }
            });

        if let Some((i, what)) = action {
            let e = self.vault.as_ref().unwrap().entries[i].clone();
            match what {
                "pass" => {
                    self.copy("password", e.password());
                    self.hide(&ctx);
                    return;
                }
                "user" => self.copy("identifier", e.user()),
                "reveal" => self.revealed = if self.revealed == Some(i) { None } else { Some(i) },
                "edit" => self.begin_edit(i),
                "delete" => self.confirm_delete = Some(i),
                _ => {}
            }
        }

        // footer: lowercase mono hints on the gutter, count on the right gutter
        let (frect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FOOT_H), Sense::hover());
        ui.painter().hline(frect.x_range(), frect.top() + 0.5, Stroke::new(1.0, p.border));
        let mono = FontId::monospace(11.0);
        let cy = frect.center().y + 0.5;
        let left = pos2(frect.left() + GUTTER, cy);
        let active_status =
            self.status.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(4)).map(|(m, _)| m.clone());
        if let Some(msg) = self.busy {
            ui.painter().text(left, Align2::LEFT_CENTER, msg.to_lowercase(), mono.clone(), p.weak);
        } else if let Some(msg) = active_status {
            ui.painter().text(left, Align2::LEFT_CENTER, msg.to_lowercase(), mono.clone(), p.ok);
            ctx.request_repaint_after(Duration::from_millis(500));
        } else {
            ui.painter().text(
                left,
                Align2::LEFT_CENTER,
                "enter copy   ^u user   ^r show   ^e edit   ^n new   ^l lock",
                mono.clone(),
                p.weak,
            );
        }
        let n = self.vault.as_ref().map_or(0, |v| v.entries.len());
        ui.painter().text(
            pos2(frect.right() - GUTTER, cy),
            Align2::RIGHT_CENTER,
            format!("{n} entries"),
            mono,
            p.weak,
        );
        if self.need_reload {
            let br = Rect::from_min_max(
                pos2(frect.right() - GUTTER - 210.0, frect.top()),
                pos2(frect.right() - GUTTER - 90.0, frect.bottom()),
            );
            ui.scope_builder(UiBuilder::new().max_rect(br).layout(Layout::right_to_left(Align::Center)), |ui| {
                if link(ui, "reload from disk", p.accent).clicked() {
                    let pw = self.pass.clone().unwrap();
                    self.start_load(pw, "Reloading…");
                }
            });
            ui.advance_cursor_after_rect(frect);
        }
    }

    fn begin_edit(&mut self, i: usize) {
        let e = &self.vault.as_ref().unwrap().entries[i];
        self.edit = Some(EditState {
            index: Some(i),
            account: e.account().into(),
            user: e.user().into(),
            password: e.password().into(),
            show_password: false,
            focus_first: true,
        });
    }

    fn ui_edit(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        let busy = self.busy.is_some();
        let is_new = self.edit.as_ref().unwrap().index.is_none();
        let mut save = false;
        let mut cancel = false;
        let mut regen = false;
        let w = PANEL_W - 2.0 * GUTTER;

        ui.add_space(22.0);
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.label(RichText::new(if is_new { "New entry" } else { "Edit entry" }).font(semibold(20.0)));
        });
        ui.add_space(16.0);
        let st = self.edit.as_mut().unwrap();
        for (label, which) in [("Account", 0), ("Identifier", 1), ("Password", 2)] {
            // label row (18px): label on the gutter, "generate" on the right gutter
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
            let lab = Rect::from_min_max(pos2(row.left() + GUTTER, row.top()), pos2(row.right() - GUTTER, row.bottom()));
            ui.painter().text(lab.left_center(), Align2::LEFT_CENTER, label, FontId::proportional(12.0), p.weak);
            if which == 2 {
                ui.scope_builder(UiBuilder::new().max_rect(lab).layout(Layout::right_to_left(Align::Center)), |ui| {
                    if link(ui, "generate", p.accent).on_hover_text("New random password (Ctrl+G)").clicked() {
                        regen = true;
                    }
                });
                ui.advance_cursor_after_rect(row);
            }
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.add_space(GUTTER);
                let te = match which {
                    0 => egui::TextEdit::singleline(&mut st.account),
                    1 => egui::TextEdit::singleline(&mut st.user),
                    _ => egui::TextEdit::singleline(&mut st.password).password(!st.show_password),
                };
                let r = ui.add(te.desired_width(w).margin(vec2(8.0, 6.0)));
                if which == 0 && st.focus_first {
                    r.request_focus();
                    st.focus_first = false;
                }
                if which == 2 && r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    save = true;
                }
            });
            ui.add_space(14.0);
        }
        if regen || ui.input(|i| i.modifiers.command && i.key_pressed(Key::G)) {
            st.password = pwgen::generate();
            st.show_password = true; // so the new password can be read and copied
        }
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.checkbox(&mut st.show_password, RichText::new("Show password").size(13.0));
        });
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.spacing_mut().item_spacing.x = 16.0;
            if flat_button(ui, "Save", p.accent, p.bg, !busy).clicked() {
                save = true;
            }
            if flat_button(ui, "Cancel", p.weak, p.bg, true).clicked() {
                cancel = true;
            }
        });
        if cancel {
            self.edit = None;
            self.focus_input = true;
        } else if save && !busy {
            let st = self.edit.take().unwrap();
            let mut nv = self.vault.clone().unwrap();
            match st.index {
                Some(i) if i < nv.entries.len() => nv.entries[i].set(st.account, st.user, st.password),
                _ => nv.entries.push(Entry::new(&st.account, &st.user, &st.password)),
            }
            self.start_save(nv);
            self.focus_input = true;
        }
    }

    fn ui_confirm_delete(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        let i = self.confirm_delete.unwrap();
        let name = self
            .vault
            .as_ref()
            .and_then(|v| v.entries.get(i))
            .map(|e| e.account().to_owned())
            .unwrap_or_default();
        ui.add_space(22.0);
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.label(RichText::new(format!("Delete “{name}”?")).font(semibold(20.0)));
        });
        ui.add_space(4.0);
        self.note(ui, "It is removed from the file immediately.");
        ui.add_space(16.0);
        let mut yes = ui.input(|i| i.key_pressed(Key::Enter));
        let mut no = false;
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.spacing_mut().item_spacing.x = 16.0;
            if flat_button(ui, "Delete", p.danger, p.bg, true).clicked() {
                yes = true;
            }
            if flat_button(ui, "Cancel", p.weak, p.bg, true).clicked() {
                no = true;
            }
        });
        if yes {
            let mut nv = self.vault.clone().unwrap();
            if i < nv.entries.len() {
                nv.entries.remove(i);
            }
            self.revealed = None;
            self.confirm_delete = None;
            self.start_save(nv);
            self.focus_input = true;
        } else if no {
            self.confirm_delete = None;
            self.focus_input = true;
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Runs before every `ui` pass and, unlike `ui`, also while the window is
    /// hidden: eframe skips the egui pass for a hidden viewport (macOS, Windows)
    /// and only calls `logic`. Commands that bring the window back must
    /// therefore be handled here, or a hidden jpass could never show again.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_command(ctx);
        self.poll_jobs();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        // Retry the focus request until the window manager grants it.
        if let Some(deadline) = self.focus_deadline {
            if !self.visible || ctx.input(|i| i.focused) || Instant::now() > deadline {
                self.focus_deadline = None;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        // Hide when focus is lost (after we have actually been focused once).
        if ctx.input(|i| i.focused) {
            self.seen_focus = true;
        } else if self.visible
            && !(cfg!(debug_assertions) && std::env::var_os("JPASS_DEV_NOHIDE").is_some())
            && self.seen_focus
            && self.shown_at.elapsed() > Duration::from_millis(400)
            && self.busy.is_none()
        {
            self.hide(&ctx);
            return;
        }

        // Esc: step back (dialog → search text → hide).
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.edit.is_some() {
                self.edit = None;
                self.focus_input = true;
            } else if self.confirm_delete.is_some() {
                self.confirm_delete = None;
                self.focus_input = true;
            } else if !self.search.is_empty() && self.vault.is_some() {
                self.search.clear();
                self.selected = 0;
            } else {
                self.hide(&ctx);
                return;
            }
        }

        // While locked, keep the drive/file state fresh (pCloud mounts late).
        if self.vault.is_none() && self.busy.is_none() {
            self.refresh_file_state();
            if !self.drive_ready {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }

        // size & position
        let list = if self.vault.is_some() { self.filtered() } else { vec![] };
        let key = self.screen_key();
        let panel_h = self.desired_height(&list);
        let win_h = panel_h + MARGIN * 2.0;
        if (win_h - self.last_height).abs() > 0.5 {
            self.last_height = win_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(WIN_W, win_h)));
        }
        if self.need_center {
            if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                    ((m.x - WIN_W) / 2.0).max(0.0),
                    (m.y * 0.2).max(0.0),
                )));
                self.need_center = false;
            }
        }

        let p = self.pal;
        let mut used = 0.0;
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let panel = Rect::from_min_size(
                ui.max_rect().min + vec2(MARGIN, MARGIN),
                vec2(PANEL_W, panel_h),
            );
            // Elm's "terminal" box: 1px border plus a hard offset shadow, no blur, no rounding.
            ui.painter().rect_filled(panel.translate(vec2(6.0, 6.0)).expand(1.0), 0.0, p.shadow);
            ui.painter().rect_filled(panel, 0.0, p.bg);
            ui.painter().rect_stroke(panel, 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

            ui.scope_builder(UiBuilder::new().max_rect(panel), |ui| {
                ui.set_clip_rect(panel);
                // Every screen spaces itself explicitly; no implicit vertical gaps.
                ui.spacing_mut().item_spacing.y = 0.0;
                if !self.error.is_empty() {
                    // fixed-height banner: up to two lines on the gutter, "dismiss" on the right gutter
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ERR_H), Sense::hover());
                    let galley = ui.painter().layout(
                        self.error.clone(),
                        FontId::proportional(13.0),
                        p.danger,
                        r.width() - 2.0 * GUTTER - 70.0,
                    );
                    let pos = pos2(r.left() + GUTTER, r.center().y - galley.size().y / 2.0);
                    ui.painter().galley(pos, galley, p.danger);
                    let dm = Rect::from_min_max(pos2(r.right() - GUTTER - 64.0, r.top()), pos2(r.right() - GUTTER, r.bottom()));
                    ui.scope_builder(UiBuilder::new().max_rect(dm).layout(Layout::right_to_left(Align::Center)), |ui| {
                        if link(ui, "dismiss", p.weak).clicked() {
                            self.error.clear();
                        }
                    });
                    ui.advance_cursor_after_rect(r);
                    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.border));
                }
                if self.vault.is_none() {
                    self.ui_locked(ui);
                } else if self.edit.is_some() {
                    self.ui_edit(ui);
                } else if self.confirm_delete.is_some() {
                    self.ui_confirm_delete(ui);
                } else {
                    self.ui_list(ui, &list);
                }
                used = ui.min_rect().bottom() - panel.top();
            });
        });
        // Remember how tall a non-list screen really was, so the next frame fits it exactly.
        if key != SCREEN_LIST && self.screen_key() == key {
            self.measured = Some((key, used));
        }
    }
}

/// `jpass --import file.csv`: merge a plaintext CSV into the encrypted vault.
fn cli_import(file: &str) -> Result<(), String> {
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

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "jpass — age-encrypted CSV password manager\n\n\
             USAGE:\n  jpass             start, or show/hide the running instance\n  \
             jpass --toggle    show/hide the running instance (bind this to a global shortcut)\n  \
             jpass --show      show the running instance\n  \
             jpass --hidden    start in the background without a window\n  \
             jpass --import F  merge entries from CSV file F (plaintext or age-encrypted) into the vault, no duplicates\n  \
             jpass --install   (re)install as an app + start at login (also done on first run)\n\n\
             File: ~/pCloudDrive/pass.rage, on macOS ~/pCloud Drive/pass.rage (created on first run if missing)\n\n\
             Built-in global hotkey (macOS / X11): Cmd+Space"
        );
        return Ok(());
    }
    // winit cannot hide/show windows on Wayland, so on Linux run through
    // XWayland (or plain X11) where Visible(false/true) really works.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        // SAFETY: single-threaded at this point (before any thread is spawned).
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
    if let Some(i) = args.iter().position(|a| a == "--import") {
        let Some(file) = args.get(i + 1) else {
            eprintln!("usage: jpass --import <file.csv>");
            std::process::exit(2);
        };
        match cli_import(file) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("jpass: {e}");
                std::process::exit(1);
            }
        }
    }
    if args.iter().any(|a| a == "--install") {
        match install::run() {
            Ok(log) => log.iter().for_each(|l| println!("{l}")),
            Err(e) => {
                eprintln!("jpass: install failed: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    let hidden = args.iter().any(|a| a == "--hidden");
    let cmd = if args.iter().any(|a| a == "--show") { "show" } else { "toggle" };
    // Only one instance may run. A second launch toggles the first (or does
    // nothing at all when it was started with --hidden, e.g. by autostart).
    let listener = match claim_socket() {
        Claim::Server(l) => l,
        Claim::AlreadyRunning => {
            if !hidden {
                send_command(cmd);
            }
            return Ok(());
        }
    };

    // First run: install as an app and start at login.
    if !install::is_installed() {
        match install::run() {
            Ok(log) => log.iter().for_each(|l| eprintln!("jpass: {l}")),
            Err(e) => eprintln!("jpass: first-run setup failed: {e}"),
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("jpass")
            .with_app_id("jpass")
            .with_inner_size([WIN_W, 200.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_type(egui::X11WindowType::Utility)
            .with_visible(!hidden)
            .with_always_on_top(),
        ..Default::default()
    };
    eframe::run_native(
        "jpass",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, hidden, listener)))),
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    /// Hide the application (what Cmd+H does). AppKit activates the
    /// previously active app, so focus returns to where the user was.
    pub fn hide_app() {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).hide(None);
        }
    }

    /// Restore a hidden application and activate it.
    pub fn unhide_app() {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).unhide(None);
        }
    }
}
