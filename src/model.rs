//! The Elm-style model: all state, the messages that change it, and the
//! pure queries on it. Effects (threads, the window, the clipboard) live in
//! `app.rs`; drawing lives in `components.rs` and the views.

use crate::components::*;
use crate::{launcher, pwgen, vault};
use age::secrecy::SecretString;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use vault::{Entry, Stamp, Vault};

/// Everything that can happen. Views and the runtime produce these;
/// `Model::update` is the only place that turns them into model changes.
pub enum Msg {
    /// From the outside: hotkey, `jpass toggle` / `show`.
    Toggle,
    Show,
    Hide,
    Job(JobResult),
    Copied { what: &'static str, ok: bool },
    Lock,
    /// Esc / Cancel: close the dialog, clear the search, or hide.
    Back,
    DismissError,
    SearchChanged,
    /// Enter on the unlock / create screen.
    Submit,
    /// Ctrl+U while locked (the view maps it to CopyUser when unlocked).
    UnlockRequest,
    /// Copy the password of this entry and hide.
    Pick(usize),
    /// Launch the app at this index and hide.
    LaunchApp(usize),
    CopyUser(usize),
    ToggleReveal(usize),
    BeginEdit(usize),
    BeginNew,
    AskDelete(usize),
    SaveEdit,
    ConfirmDelete,
    Reload,
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
    /// Spawn an application, or focus its window if one is already open.
    Launch { app: launcher::App },
    /// Rescan the installed applications; answered with `JobResult::Apps`.
    ScanApps,
}

pub enum JobResult {
    Loaded { vault: Vault, stamp: Stamp, pass: SecretString },
    Saved { vault: Vault, stamp: Stamp },
    Failed(String),
    Apps(Vec<launcher::App>),
}

/// What is on screen. `List` is always the landing screen (apps are available
/// immediately); the lock screens are reached via Ctrl+U.
#[derive(Default)]
pub enum Screen {
    /// Ask for the master passphrase of the existing vault.
    Unlock,
    /// No vault yet: choose a master passphrase.
    Create,
    #[default]
    List,
    Edit(EditState),
    ConfirmDelete(usize),
}

/// An item in the filtered search results: either a vault password or an app.
#[derive(Clone, Copy, Debug)]
pub enum Item {
    Password(usize),
    App(usize),
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

/// How long a copied secret stays on the clipboard.
pub const CLIPBOARD_CLEAR: Duration = Duration::from_secs(20);
/// After the window has been hidden this long, the vault is locked again.
pub const RELOCK_AFTER: Duration = Duration::from_secs(5 * 60);

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

    /// Discovered desktop applications (always available, even when locked).
    pub apps: Vec<launcher::App>,

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
        let mut m = Model {
            path: vault::default_path(),
            file_exists: false,
            drive_ready: false,
            pal,
            pass: None,
            vault: None,
            stamp: None,
            apps: Vec::new(),
            screen: Screen::List,
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
        };
        m.refresh_file_state();
        m
    }

    /// Apply a message. The arms that `return` ask the runtime for an effect;
    /// the rest only change the model.
    pub fn update(&mut self, msg: Msg) -> Vec<Cmd> {
        match msg {
            Msg::Toggle if self.visible => return self.hide(),
            Msg::Toggle | Msg::Show => return self.show(),
            Msg::Hide => return self.hide(),
            Msg::Back => return self.back(),
            Msg::Submit => return self.submit(),
            Msg::Reload => return self.reload(),
            Msg::LaunchApp(i) => return self.launch_app(i),
            Msg::Pick(i) => return self.pick(i),
            Msg::CopyUser(i) => return self.copy_user(i),
            Msg::SaveEdit => return self.save_edit(),
            Msg::ConfirmDelete => return self.confirm_delete(),
            Msg::Job(r) => self.job_done(r),
            Msg::Copied { what, ok } => self.copied(what, ok),
            Msg::Lock => self.lock(),
            Msg::DismissError => self.error.clear(),
            Msg::SearchChanged => self.revealed = None,
            Msg::UnlockRequest => self.unlock_request(),
            Msg::ToggleReveal(i) => self.revealed = (self.revealed != Some(i)).then_some(i),
            Msg::BeginEdit(i) => self.begin_edit(i),
            Msg::BeginNew => self.screen = Screen::Edit(EditState::new_entry()),
            Msg::AskDelete(i) => self.screen = Screen::ConfirmDelete(i),
        }
        vec![]
    }

    // ── window ──

    fn show(&mut self) -> Vec<Cmd> {
        if self.hidden_at.is_some_and(|t| t.elapsed() > RELOCK_AFTER) {
            self.lock();
        }
        self.refresh_file_state();
        self.pal = Pal::new(detect_dark());
        self.visible = true;
        self.shown_at = Instant::now();
        self.seen_focus = false;
        self.local.focus_input = true;
        self.need_center = true;
        // `Focus` is ignored while the window is still being mapped, so it is
        // re-sent from the frame loop until focus is granted.
        self.focus_deadline = Some(Instant::now() + Duration::from_millis(1500));
        let mut cmds = vec![Cmd::ShowWindow, Cmd::ScanApps];
        cmds.extend(self.pick_up_synced_changes());
        cmds
    }

    /// Reload when pCloud synced a newer file from another machine.
    fn pick_up_synced_changes(&mut self) -> Vec<Cmd> {
        let stale = self.vault.is_some() && self.busy.is_none() && !self.same_on_disk();
        if stale { self.reload() } else { vec![] }
    }

    /// Hide, dropping the search and any open dialog.
    fn hide(&mut self) -> Vec<Cmd> {
        self.visible = false;
        self.hidden_at = Some(Instant::now());
        self.local.search.clear();
        self.local.selected = 0;
        if matches!(self.screen, Screen::Edit(_) | Screen::ConfirmDelete(_)) {
            self.screen = Screen::List;
        }
        self.revealed = None;
        self.error.clear();
        vec![Cmd::HideWindow]
    }

    /// Esc: step back (dialog → list → clear search → hide).
    fn back(&mut self) -> Vec<Cmd> {
        match self.screen {
            Screen::List if self.local.search.is_empty() => return self.hide(),
            Screen::List => {
                self.local.search.clear();
                self.local.selected = 0;
            }
            Screen::Unlock | Screen::Create => {
                self.local.pass_input.clear();
                self.local.pass_confirm.clear();
                self.back_to_list();
            }
            Screen::Edit(_) | Screen::ConfirmDelete(_) => self.back_to_list(),
        }
        vec![]
    }

    fn back_to_list(&mut self) {
        self.screen = Screen::List;
        self.local.focus_input = true;
    }

    // ── the vault ──

    /// Ctrl+U while locked: the passphrase prompt, or the create one for a new vault.
    fn unlock_request(&mut self) {
        if self.vault.is_some() {
            return;
        }
        self.refresh_file_state();
        if !self.drive_ready {
            self.error = "pCloud is not mounted yet".into();
            return;
        }
        self.screen = if self.file_exists { Screen::Unlock } else { Screen::Create };
        self.local.focus_input = true;
    }

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

    /// Used by `Submit` and the dev auto-unlock.
    pub fn unlock(&mut self, pass: SecretString) -> Vec<Cmd> {
        self.begin_job("Decrypting…");
        vec![Cmd::Load { pass }]
    }

    fn reload(&mut self) -> Vec<Cmd> {
        let Some(pass) = self.pass.clone() else { return vec![] };
        self.begin_job("Reloading…");
        vec![Cmd::Load { pass }]
    }

    pub fn lock(&mut self) {
        self.pass = None;
        self.vault = None;
        self.stamp = None;
        self.local.pass_input.clear();
        self.local.pass_confirm.clear();
        self.revealed = None;
        self.need_reload = false;
        self.back_to_list();
    }

    // ── the selected row ──

    fn launch_app(&mut self, i: usize) -> Vec<Cmd> {
        let Some(app) = self.apps.get(i).cloned() else { return vec![] };
        self.then_hide(Cmd::Launch { app })
    }

    fn pick(&mut self, i: usize) -> Vec<Cmd> {
        let Some(e) = self.entry(i) else { return vec![] };
        let copy = Cmd::Copy { what: "password", text: e.password().to_owned() };
        self.then_hide(copy)
    }

    fn copy_user(&mut self, i: usize) -> Vec<Cmd> {
        let Some(e) = self.entry(i) else { return vec![] };
        vec![Cmd::Copy { what: "identifier", text: e.user().to_owned() }]
    }

    fn then_hide(&mut self, cmd: Cmd) -> Vec<Cmd> {
        let mut cmds = vec![cmd];
        cmds.extend(self.hide());
        cmds
    }

    fn copied(&mut self, what: &str, ok: bool) {
        if ok {
            self.toast(format!("Copied {what} · clears in {}s", CLIPBOARD_CLEAR.as_secs()));
        } else {
            self.error = "Could not access the clipboard".into();
        }
    }

    // ── editing ──

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

    fn save_edit(&mut self) -> Vec<Cmd> {
        let Screen::Edit(st) = std::mem::replace(&mut self.screen, Screen::List) else { return vec![] };
        let mut vault = self.vault.clone().unwrap();
        match st.index {
            Some(i) if i < vault.entries.len() => vault.entries[i].set(st.account, st.user, st.password),
            _ => vault.entries.push(Entry::new(&st.account, &st.user, &st.password)),
        }
        self.back_to_list();
        self.begin_job("Saving…");
        vec![Cmd::Save { vault }]
    }

    fn confirm_delete(&mut self) -> Vec<Cmd> {
        let Screen::ConfirmDelete(i) = self.screen else { return vec![] };
        let mut vault = self.vault.clone().unwrap();
        if i < vault.entries.len() {
            vault.entries.remove(i);
        }
        self.revealed = None;
        self.back_to_list();
        self.begin_job("Saving…");
        vec![Cmd::Save { vault }]
    }

    // ── background jobs ──

    pub fn begin_job(&mut self, msg: &'static str) {
        self.busy = Some(msg);
        self.error.clear();
    }

    fn job_done(&mut self, r: JobResult) {
        match r {
            JobResult::Apps(apps) => self.apps = apps,
            JobResult::Loaded { vault, stamp, pass } => self.loaded(vault, stamp, pass),
            JobResult::Saved { vault, stamp } => self.saved(vault, stamp),
            JobResult::Failed(e) => self.failed(e),
        }
    }

    fn loaded(&mut self, vault: Vault, stamp: Stamp, pass: SecretString) {
        self.busy = None;
        self.vault = Some(vault);
        self.stamp = Some(stamp);
        self.pass = Some(pass);
        self.local.pass_input.clear();
        self.local.pass_confirm.clear();
        self.local.selected = 0;
        self.need_reload = false;
        self.file_exists = true;
        self.back_to_list();
        self.dev_view();
    }

    fn saved(&mut self, vault: Vault, stamp: Stamp) {
        self.busy = None;
        self.vault = Some(vault);
        self.stamp = Some(stamp);
        self.toast("Saved");
    }

    fn failed(&mut self, e: String) {
        self.busy = None;
        if self.vault.is_some() && e.contains("changed on disk") {
            self.need_reload = true;
        }
        self.error = e;
    }

    /// Dev builds only: `JPASS_DEV_VIEW` jumps straight to a form for visual checks.
    #[cfg(debug_assertions)]
    fn dev_view(&mut self) {
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

    #[cfg(not(debug_assertions))]
    fn dev_view(&mut self) {}
}

// ───────────────────────────── queries ─────────────────────────────

impl Model {
    /// Apps and (when unlocked) entries matching the search, best match first, then by name.
    pub fn filtered(&self) -> Vec<Item> {
        let words: Vec<String> = self.local.search.split_whitespace().map(str::to_lowercase).collect();
        let apps = self.apps.iter().enumerate().filter_map(|(i, a)| {
            Some((Item::App(i), match_score(&a.name, &a.comment, &words)?, a.name.to_lowercase()))
        });
        let entries = self.vault.iter().flat_map(|v| v.entries.iter().enumerate()).filter_map(|(i, e)| {
            Some((Item::Password(i), match_score(e.account(), e.user(), &words)?, e.account().to_lowercase()))
        });
        let mut scored: Vec<(Item, u32, String)> = apps.chain(entries).collect();
        scored.sort_by(|a, b| (a.1, &a.2).cmp(&(b.1, &b.2)));
        scored.into_iter().map(|(item, ..)| item).collect()
    }

    pub fn entry(&self, i: usize) -> Option<&Entry> {
        self.vault.as_ref().and_then(|v| v.entries.get(i))
    }

    /// Height of the visible panel. The list is computed exactly; every other
    /// screen uses the height measured on the previous frame (so nothing can
    /// be clipped or leave dead space), with a rough estimate for the first frame.
    pub fn desired_height(&self, list: &[Item]) -> f32 {
        let err = if self.error.is_empty() { 0.0 } else { ERR_H };
        if matches!(self.screen, Screen::List) {
            return err + BAR_H + 1.0 + self.rows_h(list) + FOOT_H;
        }
        if let Some((kind, h)) = self.measured
            && kind == std::mem::discriminant(&self.screen)
        {
            return h + FORM_PAD;
        }
        let estimate = match self.screen {
            Screen::Unlock => BAR_H + 50.0,
            Screen::Create => BAR_H + 160.0,
            Screen::Edit(_) => 280.0,
            _ => 120.0,
        };
        err + FORM_PAD + estimate
    }

    /// Exact height of the rows currently shown in the list.
    pub fn rows_h(&self, list: &[Item]) -> f32 {
        list.iter().take(MAX_ROWS).map(|item| self.row_h(item)).sum::<f32>().max(ROW_H)
    }

    pub fn row_h(&self, item: &Item) -> f32 {
        match item {
            Item::Password(i) => ROW_H + if self.revealed == Some(*i) { REVEAL_EXTRA } else { 0.0 },
            Item::App(_) => ROW_H,
        }
    }

    /// Re-check the drive and the file; both can change after startup (pCloud mounts late).
    pub fn refresh_file_state(&mut self) {
        self.drive_ready = vault::drive_ready(self.path.parent().unwrap_or(Path::new(".")));
        self.file_exists = self.drive_ready && self.path.exists();
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
}

/// Score how well `words` match against a primary field (name/account) and a
/// secondary field (comment/user). Lower is better; `None` means no match.
///
///   0 — primary field starts with the query word
///   1 — a word inside the primary field starts with it
///   2 — primary field contains it as a substring
///   3 — only the secondary field contains it
///
/// Multi-word queries sum the per-word scores.
fn match_score(primary: &str, secondary: &str, words: &[String]) -> Option<u32> {
    let p = primary.to_lowercase();
    let s = secondary.to_lowercase();
    let mut total = 0;
    for w in words.iter().map(String::as_str) {
        total += if p.starts_with(w) {
            0
        } else if p.split_whitespace().any(|part| part.starts_with(w)) {
            1
        } else if p.contains(w) {
            2
        } else if s.contains(w) {
            3
        } else {
            return None;
        };
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, exec: &str, comment: &str) -> launcher::App {
        launcher::App {
            name: name.into(),
            exec: exec.into(),
            comment: comment.into(),
            icon_path: None,
            wm_class: None,
            desktop_id: exec.into(),
        }
    }

    fn unlocked() -> Model {
        let mut m = Model::new(Pal::new(true), false);
        m.vault = Some(Vault {
            header: None,
            entries: vec![Entry::new("alpha", "me", "pw1"), Entry::new("beta", "you", "pw2")],
        });
        m.pass = Some(SecretString::from("secret".to_owned()));
        m.screen = Screen::List;
        m.apps = vec![app("Firefox", "firefox", "Web Browser")];
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

    #[test]
    fn model_starts_on_list_screen() {
        let m = Model::new(Pal::new(true), false);
        assert!(matches!(m.screen, Screen::List));
    }

    #[test]
    fn lock_returns_to_list() {
        let mut m = unlocked();
        m.update(Msg::Lock);
        assert!(matches!(m.screen, Screen::List));
        assert!(m.vault.is_none());
    }

    #[test]
    fn esc_from_unlock_returns_to_list() {
        let mut m = Model::new(Pal::new(true), false);
        m.screen = Screen::Unlock;
        m.local.pass_input = "partial".into();
        assert!(m.update(Msg::Back).is_empty());
        assert!(matches!(m.screen, Screen::List));
        assert!(m.local.pass_input.is_empty());
    }

    #[test]
    fn launch_app_returns_launch_cmd_and_hides() {
        let mut m = unlocked();
        let cmds = m.update(Msg::LaunchApp(0));
        match cmds.as_slice() {
            [Cmd::Launch { app }, Cmd::HideWindow] => assert_eq!(app.exec, "firefox"),
            _ => panic!("unexpected commands: {}", cmds.len()),
        }
    }

    #[test]
    fn filtered_returns_both_apps_and_passwords() {
        let m = unlocked();
        let items = m.filtered();
        let has_app = items.iter().any(|i| matches!(i, Item::App(_)));
        let has_pw = items.iter().any(|i| matches!(i, Item::Password(_)));
        assert!(has_app, "should include apps");
        assert!(has_pw, "should include passwords");
    }

    #[test]
    fn filtered_returns_only_apps_when_locked() {
        let mut m = Model::new(Pal::new(true), false);
        m.apps = vec![app("Firefox", "firefox", "")];
        let items = m.filtered();
        assert!(items.iter().all(|i| matches!(i, Item::App(_))));
        assert!(!items.is_empty());
    }

    #[test]
    fn search_ranks_name_match_above_comment_match() {
        let mut m = Model::new(Pal::new(true), false);
        m.apps = vec![app("Meld", "meld", "Diff and merge tool"), app("Sublime Merge", "smerge", "Git client")];
        m.local.search = "merge".into();
        let items = m.filtered();
        assert_eq!(items.len(), 2);
        // "Sublime Merge" has "merge" in the name → ranks above "Meld" where it's only in the comment.
        assert!(matches!(items[0], Item::App(1)), "Sublime Merge should be first");
        assert!(matches!(items[1], Item::App(0)), "Meld should be second");
    }

    #[test]
    fn search_ranks_prefix_above_word_match() {
        let mut m = Model::new(Pal::new(true), false);
        m.apps = vec![app("Sublime Text", "subl", ""), app("Sublime Merge", "smerge", "")];
        m.local.search = "sub".into();
        let items = m.filtered();
        assert_eq!(items.len(), 2);
        // Both start with "Sub", so alphabetical within the same tier.
        assert!(matches!(items[0], Item::App(1)), "Sublime Merge first alphabetically");
        assert!(matches!(items[1], Item::App(0)), "Sublime Text second");
    }

    #[test]
    fn match_score_returns_none_when_word_missing() {
        assert!(match_score("Firefox", "Web Browser", &["zzz".into()]).is_none());
    }
}
