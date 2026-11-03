//! The Elm-style model: all state, the messages that change it, and the
//! pure queries on it. Effects (threads, the window, the clipboard) live in
//! `app.rs`; drawing lives in `components.rs` and the views.

use crate::components::*;
use crate::{pwgen, vault};
use age::secrecy::SecretString;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use vault::{Entry, Stamp, Vault};

/// How long a copied secret stays on the clipboard.
pub const CLIPBOARD_CLEAR: Duration = Duration::from_secs(20);
/// After the window has been hidden this long, the vault is locked again.
pub const RELOCK_AFTER: Duration = Duration::from_secs(5 * 60);

/// What a background job reports back.
pub enum JobResult {
    Loaded { vault: Vault, stamp: Stamp, pass: SecretString },
    Saved { vault: Vault, stamp: Stamp },
    Failed(String),
}

/// An effect for the runtime to carry out; `Model::update` returns these
/// instead of performing them.
pub enum Cmd {
    ShowWindow,
    HideWindow,
    /// Put `text` on the clipboard (and clear it later); answered with `Msg::Copied`.
    Copy { what: &'static str, text: String },
    Load { pass: SecretString },
    Create { pass: SecretString },
    /// Persist `vault`; it becomes the live vault only if the save succeeds.
    Save { vault: Vault },
}

/// What is on screen. The lock screens follow the drive/file state; the others follow the user.
#[derive(Default)]
pub enum Screen {
    /// pCloud is not mounted yet.
    #[default]
    Waiting,
    /// Ask for the master passphrase of the existing vault.
    Unlock,
    /// No vault yet: choose a master passphrase.
    Create,
    List,
    Edit(EditState),
    /// Confirm deleting the entry at this index.
    ConfirmDelete(usize),
}

/// Everything that can happen. Views and the runtime produce these;
/// `Model::update` is the only place that turns them into model changes.
pub enum Msg {
    /// From the outside: hotkey, `jpass toggle` / `show`.
    Toggle,
    Show,
    Hide,
    /// A background job finished.
    Job(JobResult),
    /// The clipboard write finished.
    Copied { what: &'static str, ok: bool },
    Lock,
    /// Esc / Cancel: close the dialog, clear the search, or hide.
    Back,
    DismissError,
    SearchChanged,
    /// Unlock or create, using the typed passphrase(s).
    Submit,
    /// Copy the password of this entry and hide.
    Pick(usize),
    CopyUser(usize),
    ToggleReveal(usize),
    BeginEdit(usize),
    BeginNew,
    AskDelete(usize),
    SaveEdit,
    ConfirmDelete,
    Reload,
}

/// Text being typed and other view-local state that widgets bind to directly.
#[derive(Default)]
pub struct Local {
    pub pass_input: String,
    pub pass_confirm: String,
    pub search: String,
    pub selected: usize,
    pub focus_input: bool,
    pub scroll_to_sel: bool,
}

#[derive(Default)]
pub struct EditState {
    /// Index into `vault.entries`; `None` = new entry.
    pub index: Option<usize>,
    pub account: String,
    pub user: String,
    pub password: String,
    pub show_password: bool,
    pub focus_first: bool,
}

impl EditState {
    /// A blank entry that starts with a freshly generated strong password.
    pub fn new_entry() -> Self {
        EditState { focus_first: true, password: pwgen::generate(), show_password: true, ..Default::default() }
    }
}

pub struct Model {
    pub path: PathBuf,
    pub file_exists: bool,
    /// The folder holding the vault is available (pCloud is mounted).
    pub drive_ready: bool,
    pub pal: Pal,

    // session
    pub pass: Option<SecretString>,
    pub vault: Option<Vault>,
    pub stamp: Option<Stamp>,

    // ui state
    pub screen: Screen,
    pub local: Local,
    pub error: String,
    pub status: Option<(String, Instant)>,
    pub revealed: Option<usize>,
    pub busy: Option<&'static str>,
    pub need_reload: bool,

    // window state
    pub visible: bool,
    pub hidden_at: Option<Instant>,
    pub shown_at: Instant,
    pub seen_focus: bool,
    /// Keep asking the window manager for focus until this instant (or until granted).
    pub focus_deadline: Option<Instant>,
    pub need_center: bool,
    pub last_height: f32,
    /// (screen kind, content height) measured on the last frame of a non-list screen.
    pub measured: Option<(std::mem::Discriminant<Screen>, f32)>,
}

impl Model {
    pub fn new(pal: Pal, start_hidden: bool) -> Self {
        let path = vault::default_path();
        Model {
            file_exists: path.exists(),
            drive_ready: vault::drive_ready(path.parent().unwrap_or(std::path::Path::new("."))),
            path,
            pal,
            pass: None,
            vault: None,
            stamp: None,
            screen: Screen::Waiting,
            local: Local { focus_input: true, ..Default::default() },
            error: String::new(),
            status: None,
            revealed: None,
            busy: None,
            need_reload: false,
            visible: !start_hidden,
            hidden_at: start_hidden.then(Instant::now),
            shown_at: Instant::now(),
            seen_focus: false,
            focus_deadline: None,
            need_center: true,
            last_height: 0.0,
            measured: None,
        }
    }

    pub fn lock(&mut self) {
        self.pass = None;
        self.vault = None;
        self.stamp = None;
        self.local.pass_input.clear();
        self.local.pass_confirm.clear();
        self.local.search.clear();
        self.revealed = None;
        self.need_reload = false;
        self.local.focus_input = true;
        self.refresh_file_state();
    }

    /// Re-check the drive and the file; both can change after startup (pCloud mounts late).
    pub fn refresh_file_state(&mut self) {
        let dir = self.path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
        self.drive_ready = vault::drive_ready(&dir);
        self.file_exists = self.drive_ready && self.path.exists();
        if self.vault.is_none() {
            self.screen = match (self.drive_ready, self.file_exists) {
                (false, _) => Screen::Waiting,
                (true, true) => Screen::Unlock,
                (true, false) => Screen::Create,
            };
        }
    }

    pub fn same_on_disk(&self) -> bool {
        match (&self.stamp, vault::stamp(&self.path)) {
            (Some(a), Some(b)) => a.same_file(&b),
            _ => false,
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    pub fn filtered(&self) -> Vec<usize> {
        let Some(v) = &self.vault else { return vec![] };
        let words: Vec<String> = self.local.search.split_whitespace().map(str::to_lowercase).collect();
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

    pub fn row_h(&self, i: usize) -> f32 {
        ROW_H + if self.revealed == Some(i) { REVEAL_EXTRA } else { 0.0 }
    }

    /// Exact height of the rows currently shown in the list.
    pub fn rows_h(&self, list: &[usize]) -> f32 {
        list.iter().take(MAX_ROWS).map(|&i| self.row_h(i)).sum::<f32>().max(ROW_H)
    }

    /// Height of the visible panel. The list is computed exactly; every other
    /// screen uses the height measured on the previous frame (so nothing can
    /// be clipped or leave dead space), with a rough estimate for the first frame.
    pub fn desired_height(&self, list: &[usize]) -> f32 {
        let err = if self.error.is_empty() { 0.0 } else { ERR_H };
        if matches!(self.screen, Screen::List) {
            return err + BAR_H + 1.0 + self.rows_h(list) + FOOT_H;
        }
        match self.measured {
            Some((k, h)) if k == std::mem::discriminant(&self.screen) => h + FORM_PAD,
            _ => {
                err + FORM_PAD
                    + match self.screen {
                        Screen::Waiting => BAR_H + 50.0,
                        Screen::Unlock => BAR_H + 60.0,
                        Screen::Create => BAR_H + 190.0,
                        Screen::Edit(_) => 330.0,
                        _ => 150.0,
                    }
            }
        }
    }

    pub fn entry(&self, i: usize) -> Option<&Entry> {
        self.vault.as_ref().and_then(|v| v.entries.get(i))
    }

    pub fn begin_edit(&mut self, i: usize) {
        let Some(e) = self.entry(i) else { return };
        self.screen = Screen::Edit(EditState {
            index: Some(i),
            account: e.account().into(),
            user: e.user().into(),
            password: e.password().into(),
            show_password: false,
            focus_first: true,
        });
    }

    // ── update ──

    pub fn update(&mut self, msg: Msg) -> Vec<Cmd> {
        match msg {
            Msg::Toggle if self.visible => self.update(Msg::Hide),
            Msg::Toggle | Msg::Show => self.show(),
            Msg::Hide => {
                self.hide();
                vec![Cmd::HideWindow]
            }
            Msg::Job(r) => {
                self.job_done(r);
                vec![]
            }
            Msg::Copied { what, ok } => {
                if ok {
                    self.toast(format!("Copied {what} · clears in {}s", CLIPBOARD_CLEAR.as_secs()));
                } else {
                    self.error = "Could not access the clipboard".into();
                }
                vec![]
            }
            Msg::Lock => {
                self.lock();
                vec![]
            }
            // Esc: step back (dialog → search text → hide).
            Msg::Back => match self.screen {
                Screen::Edit(_) | Screen::ConfirmDelete(_) => {
                    self.screen = Screen::List;
                    self.local.focus_input = true;
                    vec![]
                }
                Screen::List if !self.local.search.is_empty() => {
                    self.local.search.clear();
                    self.local.selected = 0;
                    vec![]
                }
                _ => self.update(Msg::Hide),
            },
            Msg::DismissError => {
                self.error.clear();
                vec![]
            }
            Msg::SearchChanged => {
                self.revealed = None;
                vec![]
            }
            Msg::Submit => self.submit(),
            Msg::Pick(i) => {
                let Some(text) = self.entry(i).map(|e| e.password().to_owned()) else { return vec![] };
                let mut cmds = vec![Cmd::Copy { what: "password", text }];
                cmds.extend(self.update(Msg::Hide));
                cmds
            }
            Msg::CopyUser(i) => match self.entry(i) {
                Some(e) => vec![Cmd::Copy { what: "identifier", text: e.user().to_owned() }],
                None => vec![],
            },
            Msg::ToggleReveal(i) => {
                self.revealed = if self.revealed == Some(i) { None } else { Some(i) };
                vec![]
            }
            Msg::BeginEdit(i) => {
                self.begin_edit(i);
                vec![]
            }
            Msg::BeginNew => {
                self.screen = Screen::Edit(EditState::new_entry());
                vec![]
            }
            Msg::AskDelete(i) => {
                self.screen = Screen::ConfirmDelete(i);
                vec![]
            }
            Msg::SaveEdit => {
                let Screen::Edit(st) = std::mem::replace(&mut self.screen, Screen::List) else { return vec![] };
                let mut nv = self.vault.clone().unwrap();
                match st.index {
                    Some(i) if i < nv.entries.len() => nv.entries[i].set(st.account, st.user, st.password),
                    _ => nv.entries.push(Entry::new(&st.account, &st.user, &st.password)),
                }
                self.local.focus_input = true;
                self.begin_job("Saving…");
                vec![Cmd::Save { vault: nv }]
            }
            Msg::ConfirmDelete => {
                let Screen::ConfirmDelete(i) = self.screen else { return vec![] };
                let mut nv = self.vault.clone().unwrap();
                if i < nv.entries.len() {
                    nv.entries.remove(i);
                }
                self.revealed = None;
                self.screen = Screen::List;
                self.local.focus_input = true;
                self.begin_job("Saving…");
                vec![Cmd::Save { vault: nv }]
            }
            Msg::Reload => self.reload(),
        }
    }

    /// Mark a background job as running.
    pub fn begin_job(&mut self, msg: &'static str) {
        self.busy = Some(msg);
        self.error.clear();
    }

    /// Unlock (or create) with a passphrase; used by `Submit` and the dev auto-unlock.
    pub fn unlock(&mut self, pass: SecretString) -> Vec<Cmd> {
        self.begin_job("Decrypting…");
        vec![Cmd::Load { pass }]
    }

    fn reload(&mut self) -> Vec<Cmd> {
        let Some(pass) = self.pass.clone() else { return vec![] };
        self.begin_job("Reloading…");
        vec![Cmd::Load { pass }]
    }

    /// Enter on the unlock / create screen.
    fn submit(&mut self) -> Vec<Cmd> {
        if self.local.pass_input.is_empty() {
            return vec![];
        }
        let creating = matches!(self.screen, Screen::Create);
        if creating && self.local.pass_input != self.local.pass_confirm {
            self.error = "Passphrases do not match".into();
            return vec![];
        }
        let pass = SecretString::from(std::mem::take(&mut self.local.pass_input));
        self.local.pass_confirm.clear();
        if creating {
            self.begin_job("Creating…");
            vec![Cmd::Create { pass }]
        } else {
            self.unlock(pass)
        }
    }

    fn hide(&mut self) {
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        self.local.search.clear();
        self.local.selected = 0;
        if matches!(self.screen, Screen::Edit(_) | Screen::ConfirmDelete(_)) {
            self.screen = Screen::List;
        }
        self.revealed = None;
        self.error.clear();
    }

    fn show(&mut self) -> Vec<Cmd> {
        if self.hidden_at.is_some_and(|t| t.elapsed() > RELOCK_AFTER) {
            self.lock();
        }
        self.refresh_file_state();
        // follow light/dark changes
        self.pal = Pal::new(detect_dark());

        self.visible = true;
        self.shown_at = Instant::now();
        self.seen_focus = false;
        self.local.focus_input = true;
        self.need_center = true;
        // `Focus` is ignored while the window is still being mapped, so it is
        // re-sent from the frame loop until focus is granted.
        self.focus_deadline = Some(Instant::now() + Duration::from_millis(1500));
        let mut cmds = vec![Cmd::ShowWindow];
        // Pick up changes pCloud synced from another machine.
        if self.vault.is_some() && self.busy.is_none() && !self.same_on_disk() {
            cmds.extend(self.reload());
        }
        cmds
    }

    fn job_done(&mut self, r: JobResult) {
        self.busy = None;
        match r {
            JobResult::Loaded { vault, stamp, pass } => {
                self.vault = Some(vault);
                self.stamp = Some(stamp);
                self.pass = Some(pass);
                self.local.pass_input.clear();
                self.local.pass_confirm.clear();
                self.need_reload = false;
                self.file_exists = true;
                self.screen = Screen::List;
                self.local.focus_input = true;
                // Dev builds only: jump straight to a form for visual checks.
                #[cfg(debug_assertions)]
                match std::env::var("JPASS_DEV_VIEW").as_deref() {
                    Ok("edit") => self.begin_edit(0),
                    Ok("new") => self.screen = Screen::Edit(EditState::new_entry()),
                    Ok("delete") => self.screen = Screen::ConfirmDelete(0),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An unlocked model with two entries, on the list screen.
    fn unlocked() -> Model {
        let mut m = Model::new(Pal::new(true), false);
        m.vault = Some(Vault {
            header: None,
            entries: vec![Entry::new("alpha", "me", "pw1"), Entry::new("beta", "you", "pw2")],
        });
        m.pass = Some(SecretString::from("secret".to_owned()));
        m.screen = Screen::List;
        m
    }

    #[test]
    fn esc_steps_back_from_dialog_then_search_then_hides() {
        let mut m = unlocked();
        m.update(Msg::BeginEdit(0));
        assert!(matches!(m.screen, Screen::Edit(_)));
        assert!(m.update(Msg::Back).is_empty());
        assert!(matches!(m.screen, Screen::List));

        m.local.search = "al".into();
        assert!(m.update(Msg::Back).is_empty());
        assert!(m.local.search.is_empty());

        let cmds = m.update(Msg::Back);
        assert!(matches!(cmds.as_slice(), [Cmd::HideWindow]));
        assert!(!m.visible);
    }

    #[test]
    fn pick_copies_the_password_then_hides() {
        let mut m = unlocked();
        let cmds = m.update(Msg::Pick(1));
        match cmds.as_slice() {
            [Cmd::Copy { what: "password", text }, Cmd::HideWindow] => assert_eq!(text, "pw2"),
            _ => panic!("unexpected commands"),
        }
    }

    #[test]
    fn saving_an_edit_returns_a_save_cmd_and_marks_busy() {
        let mut m = unlocked();
        m.update(Msg::BeginNew);
        if let Screen::Edit(st) = &mut m.screen {
            st.account = "gamma".into();
            st.user = "x".into();
            st.password = "pw3".into();
        }
        let cmds = m.update(Msg::SaveEdit);
        assert!(m.busy.is_some());
        assert!(matches!(m.screen, Screen::List));
        match cmds.as_slice() {
            [Cmd::Save { vault }] => assert_eq!(vault.entries.len(), 3),
            _ => panic!("unexpected commands"),
        }
        // the live vault only changes once the job reports success
        assert_eq!(m.vault.as_ref().unwrap().entries.len(), 2);
    }

    #[test]
    fn delete_needs_confirmation() {
        let mut m = unlocked();
        assert!(m.update(Msg::AskDelete(0)).is_empty());
        assert!(matches!(m.screen, Screen::ConfirmDelete(0)));
        let cmds = m.update(Msg::ConfirmDelete);
        match cmds.as_slice() {
            [Cmd::Save { vault }] => assert_eq!(vault.entries.len(), 1),
            _ => panic!("unexpected commands"),
        }
    }

    #[test]
    fn mismatched_passphrases_are_rejected_when_creating() {
        let mut m = Model::new(Pal::new(true), false);
        m.screen = Screen::Create;
        m.local.pass_input = "a".into();
        m.local.pass_confirm = "b".into();
        assert!(m.update(Msg::Submit).is_empty());
        assert_eq!(m.error, "Passphrases do not match");
        assert!(m.busy.is_none());
    }
}
