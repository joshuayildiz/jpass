//! The runtime: one window, one instance, the global hotkey, and the effects
//! the model asks for (threads, clipboard, window commands).

use crate::components::*;
use crate::model::*;
use crate::view::view;
use crate::{install, launcher, vault};
use age::secrecy::SecretString;
use eframe::egui::{self, Key, pos2, vec2};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};
use vault::{Stamp, Vault};

/// A request sent to the running instance (over the socket, or by the hotkey).
#[derive(Clone, Copy)]
pub enum Remote {
    Toggle,
    Show,
}

/// Start the GUI instance, or forward `cmd` to the one already running.
pub fn run(cmd: Remote, hidden: bool) -> eframe::Result {
    prefer_x11();
    let listener = match claim_socket() {
        Claim::Server(l) => l,
        Claim::AlreadyRunning => {
            if !hidden {
                send_command(cmd);
            }
            return Ok(());
        }
    };
    first_run_setup();
    eframe::run_native("jpass", window_options(hidden), Box::new(move |cc| Ok(Box::new(App::new(cc, hidden, listener)))))
}

/// winit cannot hide/show windows on Wayland, so on Linux run through
/// XWayland (or plain X11) where Visible(false/true) really works.
fn prefer_x11() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        // SAFETY: single-threaded at this point (before any thread is spawned).
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
}

/// First run: install as an app and start at login.
fn first_run_setup() {
    if install::is_installed() {
        return;
    }
    match install::run() {
        Ok(log) => log.iter().for_each(|l| eprintln!("jpass: {l}")),
        Err(e) => eprintln!("jpass: first-run setup failed: {e}"),
    }
}

/// A borderless, always-on-top panel that stays out of the taskbar.
fn window_options(hidden: bool) -> eframe::NativeOptions {
    let viewport = egui::ViewportBuilder::default()
        .with_title("jpass")
        .with_app_id("jpass")
        .with_inner_size([WIN_W, 200.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_taskbar(false)
        .with_window_type(egui::X11WindowType::Utility)
        .with_visible(!hidden)
        .with_always_on_top();
    eframe::NativeOptions { viewport, ..Default::default() }
}

// ───────────────────────────── the app ─────────────────────────────

struct App {
    model: Model,
    icon_cache: launcher::IconCache,
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    cmd: Pending,
    clipboard: Arc<Mutex<Option<arboard::Clipboard>>>,
    _hotkey: Option<GlobalHotKeyManager>,
}

/// The latest request not yet handled by the UI thread.
type Pending = Arc<Mutex<Option<Remote>>>;

impl App {
    fn new(cc: &eframe::CreationContext<'_>, start_hidden: bool, listener: Option<UnixListener>) -> Self {
        let cmd = Pending::default();
        serve(listener, cmd.clone(), cc.egui_ctx.clone());
        let hotkey = hotkey(&cmd, &cc.egui_ctx);
        let pal = Pal::new(detect_dark());
        install_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx, &pal);
        let (tx, rx) = channel();
        let mut app = App {
            model: Model::new(pal, start_hidden),
            icon_cache: launcher::IconCache::new(),
            tx,
            rx,
            cmd,
            clipboard: Arc::new(Mutex::new(arboard::Clipboard::new().ok())),
            _hotkey: hotkey,
        };
        app.run(&cc.egui_ctx, Cmd::ScanApps);
        app.dev_setup(&cc.egui_ctx);
        app
    }

    /// Dev builds only: prefill the search and auto-unlock for UI testing.
    #[cfg(debug_assertions)]
    fn dev_setup(&mut self, ctx: &egui::Context) {
        if let Ok(q) = std::env::var("JPASS_DEV_SEARCH") {
            self.model.local.search = q;
        }
        if let Ok(p) = std::env::var("JPASS_DEV_PASS") {
            for cmd in self.model.unlock(SecretString::from(p)) {
                self.run(ctx, cmd);
            }
        }
    }

    #[cfg(not(debug_assertions))]
    fn dev_setup(&mut self, _: &egui::Context) {}
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
        self.poll_jobs(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.model.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.keep_focus(&ctx);
        if self.hide_on_blur(&ctx) {
            return;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.dispatch(&ctx, Msg::Back);
            if !self.model.visible {
                return;
            }
        }
        self.watch_drive(&ctx);
        let list = self.model.filtered();
        let panel_h = self.model.desired_height(&list);
        self.fit_window(&ctx, panel_h);
        self.draw(ui, &list, panel_h);
    }
}

// ── one frame ──

impl App {
    /// Re-send the focus request until the window manager grants it.
    fn keep_focus(&mut self, ctx: &egui::Context) {
        let Some(deadline) = self.model.focus_deadline else { return };
        if !self.model.visible || ctx.input(|i| i.focused) || Instant::now() > deadline {
            self.model.focus_deadline = None;
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    /// Hide when focus is lost, once we have actually been focused. True when hidden.
    fn hide_on_blur(&mut self, ctx: &egui::Context) -> bool {
        if ctx.input(|i| i.focused) {
            self.model.seen_focus = true;
            return false;
        }
        let dev_nohide = cfg!(debug_assertions) && std::env::var_os("JPASS_DEV_NOHIDE").is_some();
        let blurred = self.model.visible && self.model.seen_focus && self.model.shown_at.elapsed() > Duration::from_millis(400);
        if !blurred || dev_nohide || self.model.busy.is_some() {
            return false;
        }
        self.dispatch(ctx, Msg::Hide);
        true
    }

    /// While locked, keep the drive/file state fresh (pCloud mounts late).
    fn watch_drive(&mut self, ctx: &egui::Context) {
        if self.model.vault.is_some() || self.model.busy.is_some() {
            return;
        }
        self.model.refresh_file_state();
        if !self.model.drive_ready {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    /// Size the window to the panel, and centre it on first show.
    fn fit_window(&mut self, ctx: &egui::Context, panel_h: f32) {
        let win_h = panel_h + MARGIN * 2.0;
        if (win_h - self.model.last_height).abs() > 0.5 {
            self.model.last_height = win_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(WIN_W, win_h)));
        }
        if !self.model.need_center {
            return;
        }
        let Some(m) = ctx.input(|i| i.viewport().monitor_size) else { return };
        let pos = pos2(((m.x - WIN_W) / 2.0).max(0.0), (m.y * 0.2).max(0.0));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        self.model.need_center = false;
    }

    /// Draw the current screen, then apply what the user did.
    fn draw(&mut self, ui: &mut egui::Ui, list: &[Item], panel_h: f32) {
        let ctx = ui.ctx().clone();
        let mut screen = std::mem::take(&mut self.model.screen);
        let mut local = std::mem::take(&mut self.model.local);
        let mut actions = Vec::new();
        let used = view(ui, &self.model, &mut screen, &mut local, list, panel_h, &mut self.icon_cache, &mut actions);
        let shown = std::mem::discriminant(&screen);
        self.model.screen = screen;
        self.model.local = local;
        for action in actions {
            self.dispatch(&ctx, action);
        }
        self.remember_height(shown, used);
    }

    /// A non-list screen is sized next frame from the height it really used.
    fn remember_height(&mut self, shown: std::mem::Discriminant<Screen>, used: f32) {
        let still_shown = std::mem::discriminant(&self.model.screen) == shown;
        if still_shown && shown != std::mem::discriminant(&Screen::List) {
            self.model.measured = Some((shown, used));
        }
    }
}

// ── messages in, effects out ──

impl App {
    fn handle_command(&mut self, ctx: &egui::Context) {
        let pending = self.cmd.lock().unwrap().take();
        match pending {
            Some(Remote::Toggle) => self.dispatch(ctx, Msg::Toggle),
            Some(Remote::Show) => self.dispatch(ctx, Msg::Show),
            None => {}
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        while let Ok(r) = self.rx.try_recv() {
            self.dispatch(ctx, Msg::Job(r));
        }
    }

    fn dispatch(&mut self, ctx: &egui::Context, msg: Msg) {
        for cmd in self.model.update(msg) {
            self.run(ctx, cmd);
        }
    }

    fn run(&mut self, ctx: &egui::Context, cmd: Cmd) {
        let path = self.model.path.clone();
        match cmd {
            Cmd::HideWindow => hide_window(ctx),
            Cmd::ShowWindow => show_window(ctx, &self.model.pal),
            Cmd::Copy { what, text } => self.copy(ctx, what, text),
            Cmd::Load { pass } => self.spawn_job(ctx, move || load_job(&path, pass)),
            Cmd::Create { pass } => self.spawn_job(ctx, move || create_job(&path, pass)),
            Cmd::Save { vault } => self.save(ctx, vault),
            Cmd::Launch { app } => launcher::launch(&app),
            Cmd::ScanApps => self.spawn_job(ctx, || JobResult::Apps(launcher::scan())),
        }
    }

    fn copy(&mut self, ctx: &egui::Context, what: &'static str, text: String) {
        let ok = self.clipboard.lock().unwrap().as_mut().is_some_and(|cb| cb.set_text(text.clone()).is_ok());
        self.dispatch(ctx, Msg::Copied { what, ok });
        if ok {
            clear_later(self.clipboard.clone(), text);
        }
    }

    fn save(&mut self, ctx: &egui::Context, vault: Vault) {
        let Some(pass) = self.model.pass.clone() else { return };
        let (path, stamp) = (self.model.path.clone(), self.model.stamp.clone());
        self.spawn_job(ctx, move || save_job(&path, vault, pass, stamp));
    }

    /// Run `job` off the UI thread; its result comes back through `poll_jobs`.
    fn spawn_job(&self, ctx: &egui::Context, job: impl FnOnce() -> JobResult + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }
}

// ───────────────────────────── effects ─────────────────────────────

fn show_window(ctx: &egui::Context, pal: &Pal) {
    apply_theme(ctx, pal);
    // A hidden application keeps its windows off screen even after they are
    // ordered front, so undo `hide_app` first.
    #[cfg(target_os = "macos")]
    macos::unhide_app();
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    #[cfg(not(target_os = "macos"))]
    focus_self();
}

fn hide_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    // Hiding the application too makes AppKit activate the app that was in
    // front before jpass, so the keyboard goes back where it was.
    #[cfg(target_os = "macos")]
    macos::hide_app();
}

/// Shown by `jpass toggle` rather than by a key press of our own, focus-stealing
/// prevention may only flag the window: ask the compositor directly once it is mapped.
#[cfg(not(target_os = "macos"))]
fn focus_self() {
    std::thread::spawn(|| {
        for _ in 0..5 {
            if launcher::activate("jpass", "jpass") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

/// Wipe the clipboard after `CLIPBOARD_CLEAR`, unless something else was copied meanwhile.
fn clear_later(clipboard: Arc<Mutex<Option<arboard::Clipboard>>>, text: String) {
    std::thread::spawn(move || {
        std::thread::sleep(CLIPBOARD_CLEAR);
        let mut guard = clipboard.lock().unwrap();
        let Some(cb) = guard.as_mut() else { return };
        if cb.get_text().is_ok_and(|t| t == text) {
            let _ = cb.clear();
        }
    });
}

fn load_job(path: &Path, pass: SecretString) -> JobResult {
    match vault::load(path, &pass) {
        Ok((vault, stamp)) => JobResult::Loaded { vault, stamp, pass },
        Err(e) => JobResult::Failed(e),
    }
}

fn create_job(path: &Path, pass: SecretString) -> JobResult {
    match vault::save(path, &Vault::empty(), &pass, None) {
        Ok(_) => load_job(path, pass),
        Err(e) => JobResult::Failed(e),
    }
}

fn save_job(path: &Path, vault: Vault, pass: SecretString, stamp: Option<Stamp>) -> JobResult {
    match vault::save(path, &vault, &pass, stamp.as_ref()) {
        Ok(stamp) => JobResult::Saved { vault, stamp },
        Err(e) => JobResult::Failed(e),
    }
}

// ───────────────────────────── global hotkey ─────────────────────────────

/// On KDE the hotkey is a real global shortcut running `jpass toggle`: the
/// compositor consumes it, so the focused app never sees the key (an X11 grab
/// under XWayland only gets a copy). Elsewhere it is an X11 grab: macOS and
/// X11 desktops. On other Wayland desktops, bind `jpass toggle` yourself.
fn hotkey(cmd: &Pending, ctx: &egui::Context) -> Option<GlobalHotKeyManager> {
    #[cfg(not(target_os = "macos"))]
    match install::kde_global_shortcut() {
        Some(true) => return None,
        Some(false) => {
            eprintln!("jpass: KDE shortcut registered, but Alt+Space is not bound — set a key in System Settings → Shortcuts → jpass");
            return None;
        }
        None => {}
    }
    x11_hotkey(cmd, ctx)
}

fn x11_hotkey(cmd: &Pending, ctx: &egui::Context) -> Option<GlobalHotKeyManager> {
    let mgr = GlobalHotKeyManager::new().map_err(|e| eprintln!("jpass: global hotkey unavailable: {e}")).ok()?;
    // The Cmd key: `Meta` on macOS. On this Linux keyboard layout the
    // physical Cmd key is reported as Alt (Super sits on Option).
    let mods = if cfg!(target_os = "macos") { Modifiers::META } else { Modifiers::ALT };
    let hk = HotKey::new(Some(mods), Code::Space);
    mgr.register(hk).map_err(|e| eprintln!("jpass: cannot register the global hotkey (another app may own it): {e}")).ok()?;
    let (cmd, ctx, id) = (cmd.clone(), ctx.clone(), hk.id());
    std::thread::spawn(move || {
        while let Ok(ev) = GlobalHotKeyEvent::receiver().recv() {
            if ev.id == id && ev.state == HotKeyState::Pressed {
                *cmd.lock().unwrap() = Some(Remote::Toggle);
                ctx.request_repaint();
            }
        }
    });
    Some(mgr)
}

// ───────────────────────────── single instance ─────────────────────────────

enum Claim {
    /// We own the socket: this process is the instance.
    /// `None` when the socket could not be created: run anyway, without IPC.
    Server(Option<UnixListener>),
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

fn socket_path() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(d).join("jpass.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("jpass-{user}.sock"))
}

fn send_command(cmd: Remote) -> bool {
    match UnixStream::connect(socket_path()) {
        Ok(mut s) => s.write_all(format!("{}\n", cmd.as_str()).as_bytes()).is_ok(),
        Err(_) => false,
    }
}

/// Hand every request that arrives on the socket to the UI thread.
fn serve(listener: Option<UnixListener>, cmd: Pending, ctx: egui::Context) {
    let Some(listener) = listener else { return };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let _ = BufReader::new(stream).read_line(&mut line);
            let Some(req) = Remote::parse(line.trim()) else { continue };
            *cmd.lock().unwrap() = Some(req);
            ctx.request_repaint();
        }
    });
}

impl Remote {
    fn as_str(self) -> &'static str {
        match self {
            Remote::Toggle => "toggle",
            Remote::Show => "show",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "toggle" => Some(Remote::Toggle),
            "show" => Some(Remote::Show),
            _ => None,
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    /// What Cmd+H does. AppKit activates the previously active app, so focus
    /// returns to where the user was.
    pub fn hide_app() {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).hide(None);
        }
    }

    pub fn unhide_app() {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).unhide(None);
        }
    }
}
