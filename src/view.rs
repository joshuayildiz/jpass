//! The view: `Model` in, `Msg`s out. Nothing here changes the model; widgets
//! that bind to text edit `Local` / `EditState`, which are passed in separately.

use crate::components::*;
use crate::launcher;
use crate::model::*;
use crate::pwgen;
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2,
    vec2,
};
use std::time::Duration;

/// Draw the whole panel for the current screen. Returns the content height
/// actually used, so non-list screens can be sized to fit next frame.
#[allow(clippy::too_many_arguments)] // the model, its two mutable parts, and the frame's inputs
pub fn view(
    ui: &mut egui::Ui,
    m: &Model,
    screen: &mut Screen,
    local: &mut Local,
    list: &[Item],
    panel_h: f32,
    icon_cache: &mut launcher::IconCache,
    out: &mut Vec<Msg>,
) -> f32 {
    let mut used = 0.0;
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
        let panel = Rect::from_min_size(ui.max_rect().min + vec2(MARGIN, MARGIN), vec2(PANEL_W, panel_h));
        panel_box(ui.painter(), &m.pal, panel);
        ui.scope_builder(UiBuilder::new().max_rect(panel), |ui| {
            ui.set_clip_rect(panel);
            ui.spacing_mut().item_spacing.y = 0.0;
            if !m.error.is_empty() && error_banner(ui, &m.pal, &m.error) {
                out.push(Msg::DismissError);
            }
            match screen {
                Screen::Unlock => view_unlock(m, ui, local, false, out),
                Screen::Create => view_unlock(m, ui, local, true, out),
                Screen::List => view_list(m, ui, local, list, icon_cache, out),
                Screen::Edit(st) => view_edit(m, ui, st, out),
                Screen::ConfirmDelete(i) => view_confirm_delete(m, ui, *i, out),
            }
            used = ui.min_rect().bottom() - panel.top();
        });
    });
    used
}

/// Elm's "terminal" box: 1px border plus a hard offset shadow, no blur, no rounding.
fn panel_box(painter: &egui::Painter, p: &Pal, panel: Rect) {
    painter.rect_filled(panel.translate(vec2(6.0, 6.0)).expand(1.0), 0.0, p.shadow);
    painter.rect_filled(panel, 0.0, p.bg);
    painter.rect_stroke(panel, 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
}

/// The input bar bound to the search text, or to the passphrase when locked.
fn input_bar(m: &Model, ui: &mut egui::Ui, local: &mut Local, hint: &str, locked: bool, lock_state: Option<bool>) -> egui::Response {
    let text = if locked { &mut local.pass_input } else { &mut local.search };
    search_bar(ui, &m.pal, text, &mut local.focus_input, hint, locked, lock_state)
}

// ───────────────────────────── unlock / create ─────────────────────────────

fn view_unlock(m: &Model, ui: &mut egui::Ui, local: &mut Local, creating: bool, out: &mut Vec<Msg>) {
    let busy = m.busy.is_some();
    let hint = if creating { "Choose a master passphrase" } else { "Master passphrase" };
    let first = ui.add_enabled_ui(!busy, |ui| input_bar(m, ui, local, hint, true, None)).inner;
    separator(ui, &m.pal);
    ui.add_space(10.0);
    let confirm = creating.then(|| confirm_field(m, ui, local, busy));
    match m.busy {
        Some(msg) => spinner_line(ui, &m.pal, msg),
        None => hints(ui, &m.pal, if creating { "enter create   esc back" } else { "enter unlock   esc back" }),
    }
    if submitted(ui, &first, confirm.as_ref()) {
        out.push(Msg::Submit);
    }
}

/// The "repeat passphrase" field of the create screen, under the path the vault will get.
fn confirm_field(m: &Model, ui: &mut egui::Ui, local: &mut Local, busy: bool) -> egui::Response {
    note(ui, &m.pal, "No password file yet. A new encrypted one will be created at");
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.label(RichText::new(tilde(&m.path.display().to_string())).font(FontId::monospace(SMALL_FONT)));
    });
    ui.add_space(8.0);
    let edit = egui::TextEdit::singleline(&mut local.pass_confirm)
        .password(true)
        .hint_text("Repeat passphrase")
        .margin(vec2(8.0, 5.0))
        .desired_width(PANEL_W - 2.0 * GUTTER);
    let r = ui
        .horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.add_enabled(!busy, edit)
        })
        .inner;
    ui.add_space(10.0);
    r
}

/// Enter in the last field submits; in the first one it moves on to the confirmation.
fn submitted(ui: &egui::Ui, first: &egui::Response, confirm: Option<&egui::Response>) -> bool {
    let enter = ui.input(|i| i.key_pressed(Key::Enter));
    match confirm {
        None => first.lost_focus() && enter,
        Some(c) if first.lost_focus() && enter => {
            c.request_focus();
            false
        }
        Some(c) => c.lost_focus() && enter,
    }
}

// ───────────────────────────── list ─────────────────────────────

fn view_list(m: &Model, ui: &mut egui::Ui, local: &mut Local, list: &[Item], icon_cache: &mut launcher::IconCache, out: &mut Vec<Msg>) {
    let locked = m.vault.is_none();
    let r = input_bar(m, ui, local, "Search", false, Some(locked));
    if r.changed() {
        local.selected = 0;
        out.push(Msg::SearchChanged);
    }
    separator(ui, &m.pal);
    local.selected = local.selected.min(list.len().saturating_sub(1));
    if m.busy.is_none() {
        list_keys(ui, local, list, locked, out);
    }
    list_rows(m, ui, local, list, icon_cache, out);
    list_footer(m, ui, local, list, out);
}

/// Arrows move the selection; every other key maps straight to a message.
fn list_keys(ui: &egui::Ui, local: &mut Local, list: &[Item], locked: bool, out: &mut Vec<Msg>) {
    let key = |k: Key| ui.input(|i| i.key_pressed(k));
    let ctrl = |k: Key| ui.input(|i| i.modifiers.command && i.key_pressed(k));
    if key(Key::ArrowDown) {
        step_selection(local, list, 1);
    }
    if key(Key::ArrowUp) {
        step_selection(local, list, -1);
    }
    let general = [
        (ctrl(Key::U) && locked, Msg::UnlockRequest),
        (ctrl(Key::L) && !locked, Msg::Lock),
        (ctrl(Key::N) && !locked, Msg::BeginNew),
    ];
    let on_row = match list.get(local.selected) {
        Some(&Item::Password(i)) => vec![
            (key(Key::Enter), Msg::Pick(i)),
            (ctrl(Key::U), Msg::CopyUser(i)),
            (ctrl(Key::R), Msg::ToggleReveal(i)),
            (ctrl(Key::E), Msg::BeginEdit(i)),
            (ctrl(Key::D), Msg::AskDelete(i)),
        ],
        Some(&Item::App(i)) => vec![(key(Key::Enter), Msg::LaunchApp(i))],
        None => vec![],
    };
    let pressed = general.into_iter().chain(on_row).filter_map(|(down, msg)| down.then_some(msg));
    out.extend(pressed);
}

fn step_selection(local: &mut Local, list: &[Item], step: isize) {
    let Some(next) = local.selected.checked_add_signed(step).filter(|&n| n < list.len()) else { return };
    local.selected = next;
    local.scroll_to_sel = true;
}

fn list_rows(m: &Model, ui: &mut egui::Ui, local: &mut Local, list: &[Item], icon_cache: &mut launcher::IconCache, out: &mut Vec<Msg>) {
    egui::ScrollArea::vertical()
        .max_height(m.rows_h(list))
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if list.is_empty() {
                empty_row(m, ui);
            }
            for (index, item) in list.iter().enumerate() {
                let row = row_frame(m, ui, local, index, item);
                match *item {
                    Item::Password(i) => password_row(m, ui, local, &row, i, out),
                    Item::App(i) => app_row(m, ui, local, icon_cache, &row, i, out),
                }
            }
        });
}

fn empty_row(m: &Model, ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, empty_message(m), FontId::proportional(SMALL_FONT), m.pal.weak);
}

fn empty_message(m: &Model) -> &'static str {
    let no_entries = m.vault.as_ref().is_some_and(|v| v.entries.is_empty());
    match (m.apps.is_empty(), m.vault.is_none(), no_entries) {
        (true, true, _) => "No applications found",
        (true, false, true) => "No entries yet \u{2014} ctrl+n to add one",
        _ => "No matching entries",
    }
}

// ── rows ──

fn password_row(m: &Model, ui: &mut egui::Ui, local: &mut Local, row: &Row, i: usize, out: &mut Vec<Msg>) {
    let Some(e) = m.entry(i) else { return };
    let p = m.pal;
    let body = Rect::from_center_size(row.icon_box().center() + vec2(0.0, 2.5), vec2(14.0, 10.0));
    padlock(ui.painter(), body, 4.8, false, Stroke::new(1.4, p.weak));
    let (x, cy) = (row.text_x(), row.cy());
    let revealed = m.revealed == Some(i);
    let (ya, yb) = if revealed { (cy - 15.0, cy - 0.5) } else { (cy - 8.0, cy + 7.5) };
    ui.painter().text(pos2(x, ya), Align2::LEFT_CENTER, e.account(), semibold(TITLE_FONT), p.text);
    ui.painter().text(pos2(x, yb), Align2::LEFT_CENTER, e.user(), FontId::monospace(SUB_FONT), p.weak);
    if revealed {
        ui.painter().text(pos2(x, cy + 14.0), Align2::LEFT_CENTER, e.password(), FontId::monospace(SUB_FONT), p.accent);
    }
    row_clicks(local, row, Msg::Pick(i), out);
    let links = vec![
        ("delete", Msg::AskDelete(i), p.danger),
        ("edit", Msg::BeginEdit(i), p.accent),
        ("show", Msg::ToggleReveal(i), p.accent),
        ("user", Msg::CopyUser(i), p.accent),
        ("copy", Msg::Pick(i), p.accent),
    ];
    row_links(ui, row, 300.0, links, out);
}

fn app_row(
    m: &Model,
    ui: &mut egui::Ui,
    local: &mut Local,
    icon_cache: &mut launcher::IconCache,
    row: &Row,
    i: usize,
    out: &mut Vec<Msg>,
) {
    let app = &m.apps[i];
    let p = m.pal;
    match icon_cache.get(ui.ctx(), app) {
        Some(tex) => {
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            ui.painter().image(tex.id(), row.icon_box(), uv, Color32::WHITE);
        }
        None => grid_glyph(ui.painter(), row.icon_box().center(), Stroke::new(1.4, p.weak)),
    }
    let (x, cy) = (row.text_x(), row.cy());
    let subtitle = if app.comment.is_empty() { "app" } else { &app.comment };
    ui.painter().text(pos2(x, cy - 8.0), Align2::LEFT_CENTER, &app.name, semibold(TITLE_FONT), p.text);
    ui.painter().text(pos2(x, cy + 7.5), Align2::LEFT_CENTER, subtitle, FontId::proportional(SUB_FONT), p.weak);
    row_clicks(local, row, Msg::LaunchApp(i), out);
    row_links(ui, row, 90.0, vec![("launch", Msg::LaunchApp(i), p.ok)], out);
}

/// One allocated list row and the geometry every row shares.
struct Row {
    rect: Rect,
    resp: egui::Response,
    sel: bool,
    index: usize,
}

impl Row {
    fn text_x(&self) -> f32 {
        self.rect.left() + TEXT_X
    }

    fn cy(&self) -> f32 {
        self.rect.center().y
    }

    fn icon_box(&self) -> Rect {
        Rect::from_min_size(pos2(self.rect.left() + GUTTER, self.cy() - ICON / 2.0), vec2(ICON, ICON))
    }
}

/// Allocate the row and paint its selection or hover background.
fn row_frame(m: &Model, ui: &mut egui::Ui, local: &mut Local, index: usize, item: &Item) -> Row {
    let sel = index == local.selected;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), m.row_h(item)), Sense::click());
    if sel {
        selected_background(ui.painter(), &m.pal, rect, item);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, m.pal.sel.gamma_multiply(0.55));
    }
    if sel && std::mem::take(&mut local.scroll_to_sel) {
        resp.scroll_to_me(None);
    }
    Row { rect, resp, sel, index }
}

/// Selection fill with a 3px bar on the left: accent for passwords, green for apps.
fn selected_background(painter: &egui::Painter, p: &Pal, rect: Rect, item: &Item) {
    let bar = match item {
        Item::Password(_) => p.accent,
        Item::App(_) => p.ok,
    };
    painter.rect_filled(rect, 0.0, p.sel);
    painter.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, bar);
}

/// A click selects the row; a double click is the row's `primary` action.
fn row_clicks(local: &mut Local, row: &Row, primary: Msg, out: &mut Vec<Msg>) {
    if row.resp.double_clicked() {
        local.selected = row.index;
        out.push(primary);
    } else if row.resp.clicked() {
        local.selected = row.index;
    }
}

/// Text links on the right of a selected or hovered row, `width` px from its right edge.
fn row_links(ui: &mut egui::Ui, row: &Row, width: f32, links: Vec<(&str, Msg, Color32)>, out: &mut Vec<Msg>) {
    if !(row.sel || row.resp.hovered()) {
        return;
    }
    let area = Rect::from_min_max(
        pos2(row.rect.right() - width, row.rect.top()),
        pos2(row.rect.right() - GUTTER, row.rect.top() + ROW_H),
    );
    ui.scope_builder(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for (label, msg, color) in links {
            if link(ui, label, color).clicked() {
                out.push(msg);
            }
        }
    });
    ui.advance_cursor_after_rect(row.rect);
}

// ── footer ──

fn list_footer(m: &Model, ui: &mut egui::Ui, local: &Local, list: &[Item], out: &mut Vec<Msg>) {
    let p = m.pal;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FOOT_H), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
    let mono = FontId::monospace(HINT_FONT);
    let cy = rect.center().y + 0.5;
    let (status, color) = footer_status(m, ui, local, list);
    ui.painter().text(pos2(rect.left() + GUTTER, cy), Align2::LEFT_CENTER, status, mono.clone(), color);
    ui.painter().text(pos2(rect.right() - GUTTER, cy), Align2::RIGHT_CENTER, footer_count(m), mono, p.weak);
    if m.need_reload {
        reload_link(ui, &p, rect, out);
    }
}

/// Left side of the footer: the running job, a fresh toast, or the key hints.
fn footer_status(m: &Model, ui: &egui::Ui, local: &Local, list: &[Item]) -> (String, Color32) {
    let p = m.pal;
    if let Some(msg) = m.busy {
        return (msg.to_lowercase(), p.weak);
    }
    if let Some((msg, at)) = &m.status
        && at.elapsed() < Duration::from_secs(4)
    {
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        return (msg.to_lowercase(), p.ok);
    }
    let on_app = matches!(list.get(local.selected), Some(Item::App(_)));
    let hint = match (m.vault.is_none(), on_app) {
        (true, _) => "enter launch   ^u unlock",
        (false, true) => "enter launch   ^n new   ^l lock",
        (false, false) => "enter pick   ^u user   ^r show   ^e edit   ^n new   ^l lock",
    };
    (hint.to_string(), p.weak)
}

fn footer_count(m: &Model) -> String {
    let apps = m.apps.len();
    match &m.vault {
        None => format!("{apps} apps"),
        Some(v) => format!("{} passwords \u{00b7} {apps} apps", v.entries.len()),
    }
}

fn reload_link(ui: &mut egui::Ui, p: &Pal, footer: Rect, out: &mut Vec<Msg>) {
    let area = Rect::from_min_max(
        pos2(footer.right() - GUTTER - 200.0, footer.top()),
        pos2(footer.right() - GUTTER - 85.0, footer.bottom()),
    );
    ui.scope_builder(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)), |ui| {
        if link(ui, "reload from disk", p.accent).clicked() {
            out.push(Msg::Reload);
        }
    });
    ui.advance_cursor_after_rect(footer);
}

// ───────────────────────────── forms ─────────────────────────────

fn view_edit(m: &Model, ui: &mut egui::Ui, st: &mut EditState, out: &mut Vec<Msg>) {
    let p = m.pal;
    let busy = m.busy.is_some();
    form_title(ui, if st.index.is_none() { "New entry" } else { "Edit entry" });
    ui.add_space(12.0);
    let (regen, submit) = edit_fields(ui, &p, st);
    if regen || ui.input(|i| i.modifiers.command && i.key_pressed(Key::G)) {
        st.password = pwgen::generate();
        st.show_password = true;
    }
    if submit && !busy {
        out.push(Msg::SaveEdit);
    }
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.checkbox(&mut st.show_password, RichText::new("Show password").size(SMALL_FONT));
    });
    ui.add_space(12.0);
    form_buttons(ui, &p, "Save", p.accent, !busy, Msg::SaveEdit, out);
}

/// The three fields of the form. Returns (generate clicked, Enter in the last field).
fn edit_fields(ui: &mut egui::Ui, p: &Pal, st: &mut EditState) -> (bool, bool) {
    let mut regen = false;
    let masked = !st.show_password;
    let account = edit_field(ui, p, "Account", &mut st.account, false, None);
    edit_field(ui, p, "Identifier", &mut st.user, false, None);
    let password = edit_field(ui, p, "Password", &mut st.password, masked, Some(&mut regen));
    if std::mem::take(&mut st.focus_first) {
        account.request_focus();
    }
    let submit = password.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    (regen, submit)
}

/// A labelled text field; with `generate`, a "generate" link on the label row sets it.
fn edit_field(
    ui: &mut egui::Ui,
    p: &Pal,
    label: &str,
    value: &mut String,
    masked: bool,
    generate: Option<&mut bool>,
) -> egui::Response {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    let lab = Rect::from_min_max(pos2(row.left() + GUTTER, row.top()), pos2(row.right() - GUTTER, row.bottom()));
    ui.painter().text(lab.left_center(), Align2::LEFT_CENTER, label, FontId::proportional(SUB_FONT), p.weak);
    if let Some(regen) = generate {
        ui.scope_builder(UiBuilder::new().max_rect(lab).layout(Layout::right_to_left(Align::Center)), |ui| {
            if link(ui, "generate", p.accent).on_hover_text("New random password (Ctrl+G)").clicked() {
                *regen = true;
            }
        });
        ui.advance_cursor_after_rect(row);
    }
    ui.add_space(2.0);
    let edit = egui::TextEdit::singleline(value).password(masked).desired_width(PANEL_W - 2.0 * GUTTER).margin(vec2(8.0, 5.0));
    let r = ui
        .horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.add(edit)
        })
        .inner;
    ui.add_space(10.0);
    r
}

fn view_confirm_delete(m: &Model, ui: &mut egui::Ui, i: usize, out: &mut Vec<Msg>) {
    let p = m.pal;
    let name = m.entry(i).map(|e| e.account().to_owned()).unwrap_or_default();
    form_title(ui, format!("Delete “{name}”?"));
    ui.add_space(4.0);
    note(ui, &m.pal, "It is removed from the file immediately.");
    ui.add_space(12.0);
    if ui.input(|i| i.key_pressed(Key::Enter)) {
        out.push(Msg::ConfirmDelete);
    }
    form_buttons(ui, &p, "Delete", p.danger, true, Msg::ConfirmDelete, out);
}

fn form_title(ui: &mut egui::Ui, title: impl Into<String>) {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.label(RichText::new(title).font(semibold(18.0)));
    });
}

/// The form's action button, next to "Cancel".
fn form_buttons(ui: &mut egui::Ui, p: &Pal, label: &str, color: Color32, enabled: bool, action: Msg, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.spacing_mut().item_spacing.x = 12.0;
        if flat_button(ui, label, color, p.bg, enabled).clicked() {
            out.push(action);
        }
        if flat_button(ui, "Cancel", p.weak, p.bg, true).clicked() {
            out.push(Msg::Back);
        }
    });
}
