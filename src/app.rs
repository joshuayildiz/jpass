
use age::secrecy::SecretString;
use eframe::egui::{self, Key, pos2, vec2};
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
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};
use crate::components::*;
use crate::model::*;
use crate::view::view;
use crate::{install, vault};
use vault::Vault;

/// A request sent to the running instance (over the socket, or by the hotkey).
#[derive(Clone, Copy)]
pub enum Remote {
    Toggle,
    Show,
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

/// The latest request not yet handled by the UI thread.
type Pending = Arc<Mutex<Option<Remote>>>;

// ───────────────────────────── single instance / IPC ─────────────────────────────

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

// ───────────────────────────── app ─────────────────────────────

struct App {
    model: Model,

    // plumbing
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    cmd: Pending,
    clipboard: Arc<Mutex<Option<arboard::Clipboard>>>,
    _hotkey: Option<GlobalHotKeyManager>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, start_hidden: bool, listener: Option<UnixListener>) -> Self {
        let cmd = Pending::default();
        serve(listener, cmd.clone(), cc.egui_ctx.clone());

        // Built-in global hotkey (macOS + X11). Everywhere else, bind
        // `jpass toggle` to a key in the desktop's shortcut settings.
        let mut hk_mgr = None;
        match GlobalHotKeyManager::new() {
            Err(e) => eprintln!("jpass: global hotkey unavailable: {e}"),
            Ok(mgr) => {
                // The Cmd key: `Meta` on macOS. On this Linux keyboard layout the
                // physical Cmd key is reported as Alt (Super sits on Option).
                #[cfg(target_os = "macos")]
                let mods = Modifiers::META;
                #[cfg(not(target_os = "macos"))]
                let mods = Modifiers::ALT;
                let hk = HotKey::new(Some(mods), Code::Space);
                match mgr.register(hk) {
                    Err(e) => eprintln!("jpass: cannot register the global hotkey (another app may own it): {e}"),
                    Ok(()) => {
                        let c = cmd.clone();
                        let ctx = cc.egui_ctx.clone();
                        let id = hk.id();
                        std::thread::spawn(move || {
                            while let Ok(ev) = GlobalHotKeyEvent::receiver().recv() {
                                if ev.id == id && ev.state == HotKeyState::Pressed {
                                    *c.lock().unwrap() = Some(Remote::Toggle);
                                    ctx.request_repaint();
                                }
                            }
                        });
                        hk_mgr = Some(mgr);
                    }
                }
            }
        }

        let pal = Pal::new(detect_dark());
        install_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx, &pal);

        let (tx, rx) = channel();
        #[allow(unused_mut)]
        let mut app = App {
            model: Model::new(pal, start_hidden),
            tx,
            rx,
            cmd,
            clipboard: Arc::new(Mutex::new(arboard::Clipboard::new().ok())),
            _hotkey: hk_mgr,
        };
        app.model.refresh_file_state();
        #[cfg(debug_assertions)]
        if let Ok(q) = std::env::var("JPASS_DEV_SEARCH") {
            app.model.local.search = q;
        }
        // Dev builds only: auto-unlock for UI testing. Compiled out of release builds.
        #[cfg(debug_assertions)]
        if let Ok(p) = std::env::var("JPASS_DEV_PASS") {
            for cmd in app.model.unlock(SecretString::from(p)) {
                app.run(&cc.egui_ctx, cmd);
            }
        }
        app
    }

    // ── session control ──

    fn handle_command(&mut self, ctx: &egui::Context) {
        let pending = self.cmd.lock().unwrap().take();
        match pending {
            Some(Remote::Toggle) => self.dispatch(ctx, Msg::Toggle),
            Some(Remote::Show) => self.dispatch(ctx, Msg::Show),
            None => {}
        }
    }

    // ── runtime: feed messages to the model, then carry out the effects it asks for ──

    fn dispatch(&mut self, ctx: &egui::Context, msg: Msg) {
        for cmd in self.model.update(msg) {
            self.run(ctx, cmd);
        }
    }

    fn run(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::HideWindow => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                // Also hide the application itself: AppKit then activates the app that
                // was in front before jpass, so the keyboard goes back where it was.
                #[cfg(target_os = "macos")]
                macos::hide_app();
            }
            Cmd::ShowWindow => {
                apply_theme(ctx, &self.model.pal);
                // Undo `hide_app`: a hidden application keeps its windows off screen
                // even after they are ordered front, so unhide before showing.
                #[cfg(target_os = "macos")]
                macos::unhide_app();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            }
            Cmd::Copy { what, text } => self.copy(ctx, what, text),
            Cmd::Load { pass } => {
                let (tx, path) = (self.tx.clone(), self.model.path.clone());
                std::thread::spawn(move || {
                    let r = match vault::load(&path, &pass) {
                        Ok((vault, stamp)) => JobResult::Loaded { vault, stamp, pass },
                        Err(e) => JobResult::Failed(e),
                    };
                    let _ = tx.send(r);
                });
            }
            Cmd::Create { pass } => {
                let (tx, path) = (self.tx.clone(), self.model.path.clone());
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
            Cmd::Save { vault: new_vault } => {
                let Some(pass) = self.model.pass.clone() else { return };
                let (tx, path, stamp) = (self.tx.clone(), self.model.path.clone(), self.model.stamp.clone());
                std::thread::spawn(move || {
                    let r = match vault::save(&path, &new_vault, &pass, stamp.as_ref()) {
                        Ok(stamp) => JobResult::Saved { vault: new_vault, stamp },
                        Err(e) => JobResult::Failed(e),
                    };
                    let _ = tx.send(r);
                });
            }
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        while let Ok(r) = self.rx.try_recv() {
            self.dispatch(ctx, Msg::Job(r));
        }
    }

    fn copy(&mut self, ctx: &egui::Context, what: &'static str, text: String) {
        let ok = {
            let mut guard = self.clipboard.lock().unwrap();
            guard.as_mut().is_some_and(|cb| cb.set_text(text.clone()).is_ok())
        };
        self.dispatch(ctx, Msg::Copied { what, ok });
        if !ok {
            return;
        }
        let cb = self.clipboard.clone();
        std::thread::spawn(move || {
            std::thread::sleep(CLIPBOARD_CLEAR);
            if let Some(cb) = cb.lock().unwrap().as_mut() {
                if cb.get_text().is_ok_and(|t| t == text) {
                    let _ = cb.clear();
                }
            }
        });
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
        self.poll_jobs(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.model.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        // Retry the focus request until the window manager grants it.
        if let Some(deadline) = self.model.focus_deadline {
            if !self.model.visible || ctx.input(|i| i.focused) || Instant::now() > deadline {
                self.model.focus_deadline = None;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        // Hide when focus is lost (after we have actually been focused once).
        if ctx.input(|i| i.focused) {
            self.model.seen_focus = true;
        } else if self.model.visible
            && !(cfg!(debug_assertions) && std::env::var_os("JPASS_DEV_NOHIDE").is_some())
            && self.model.seen_focus
            && self.model.shown_at.elapsed() > Duration::from_millis(400)
            && self.model.busy.is_none()
        {
            self.dispatch(&ctx, Msg::Hide);
            return;
        }

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.dispatch(&ctx, Msg::Back);
            if !self.model.visible {
                return;
            }
        }

        // While locked, keep the drive/file state fresh (pCloud mounts late).
        if self.model.vault.is_none() && self.model.busy.is_none() {
            self.model.refresh_file_state();
            if !self.model.drive_ready {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }

        // size & position
        let list = if self.model.vault.is_some() { self.model.filtered() } else { vec![] };
        let panel_h = self.model.desired_height(&list);
        let win_h = panel_h + MARGIN * 2.0;
        if (win_h - self.model.last_height).abs() > 0.5 {
            self.model.last_height = win_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(WIN_W, win_h)));
        }
        if self.model.need_center {
            if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                    ((m.x - WIN_W) / 2.0).max(0.0),
                    (m.y * 0.2).max(0.0),
                )));
                self.model.need_center = false;
            }
        }

        let mut actions = Vec::new();
        // The view reads the model, so the screen and the typed text are borrowed out of it meanwhile.
        let mut screen = std::mem::take(&mut self.model.screen);
        let mut local = std::mem::take(&mut self.model.local);
        let used = view(ui, &self.model, &mut screen, &mut local, &list, panel_h, &mut actions);
        let shown = std::mem::discriminant(&screen);
        let was_list = matches!(screen, Screen::List);
        self.model.screen = screen;
        self.model.local = local;
        for action in actions {
            self.dispatch(&ctx, action);
        }
        // Remember how tall a non-list screen really was, so the next frame fits it exactly.
        if !was_list && std::mem::discriminant(&self.model.screen) == shown {
            self.model.measured = Some((shown, used));
        }
    }
}

/// Start the GUI instance, or forward `cmd` to the one already running.
pub fn run(cmd: Remote, hidden: bool) -> eframe::Result {
    // winit cannot hide/show windows on Wayland, so on Linux run through
    // XWayland (or plain X11) where Visible(false/true) really works.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        // SAFETY: single-threaded at this point (before any thread is spawned).
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
    // Only one instance may run. A second launch toggles the first (or does
    // nothing at all when it was started with `hidden`, e.g. by autostart).
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
