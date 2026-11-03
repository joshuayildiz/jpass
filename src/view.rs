//! The view: `Model` in, `Msg`s out. Nothing here changes the model; widgets
//! that bind to text edit `Local` / `EditState`, which are passed in separately.

use crate::components::*;
use crate::model::*;
use crate::pwgen;
use eframe::egui::{
    self, Align, Align2, FontId, Key, Layout, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2,
    vec2,
};
use std::time::Duration;

/// Draw the whole panel for the current screen. Returns the content height
/// actually used, so non-list screens can be sized to fit next frame.
pub fn view(
    ui: &mut egui::Ui,
    m: &Model,
    screen: &mut Screen,
    local: &mut Local,
    list: &[usize],
    panel_h: f32,
    out: &mut Vec<Msg>,
) -> f32 {
    let p = m.pal;
    let mut used = 0.0;
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
        let panel = Rect::from_min_size(ui.max_rect().min + vec2(MARGIN, MARGIN), vec2(PANEL_W, panel_h));
        // Elm's "terminal" box: 1px border plus a hard offset shadow, no blur, no rounding.
        ui.painter().rect_filled(panel.translate(vec2(6.0, 6.0)).expand(1.0), 0.0, p.shadow);
        ui.painter().rect_filled(panel, 0.0, p.bg);
        ui.painter().rect_stroke(panel, 0.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

        ui.scope_builder(UiBuilder::new().max_rect(panel), |ui| {
            ui.set_clip_rect(panel);
            // Every screen spaces itself explicitly; no implicit vertical gaps.
            ui.spacing_mut().item_spacing.y = 0.0;
            if !m.error.is_empty() && error_banner(ui, &p, &m.error) {
                out.push(Msg::DismissError);
            }
            match screen {
                Screen::Waiting => view_waiting(m, ui, local),
                Screen::Unlock => view_unlock(m, ui, local, false, out),
                Screen::Create => view_unlock(m, ui, local, true, out),
                Screen::List => view_list(m, ui, local, list, out),
                Screen::Edit(st) => view_edit(m, ui, st, out),
                Screen::ConfirmDelete(i) => view_confirm_delete(m, ui, *i, out),
            }
            used = ui.min_rect().bottom() - panel.top();
        });
    });
    used
}

/// The input bar bound to the search text, or to the passphrase when locked.
fn input_bar(m: &Model, ui: &mut egui::Ui, local: &mut Local, hint: &str, locked: bool) -> egui::Response {
    let text = if locked { &mut local.pass_input } else { &mut local.search };
    search_bar(ui, &m.pal, text, &mut local.focus_input, hint, locked)
}

/// pCloud is not mounted yet: never offer to create a vault in the bare mount point.
fn view_waiting(m: &Model, ui: &mut egui::Ui, local: &mut Local) {
    ui.add_enabled_ui(false, |ui| {
        input_bar(m, ui, local, "Waiting for pCloud…", true);
    });
    local.focus_input = true;
    separator(ui, &m.pal);
    ui.add_space(14.0);
    let dir = m.path.parent().map(|d| d.display().to_string()).unwrap_or_default();
    spinner_line(ui, &m.pal, &format!("Waiting for {} to be mounted…", tilde(&dir)));
}

/// Passphrase prompt; with `creating`, also asks for a confirmation.
fn view_unlock(m: &Model, ui: &mut egui::Ui, local: &mut Local, creating: bool, out: &mut Vec<Msg>) {
    let busy = m.busy.is_some();
    let r = ui
        .add_enabled_ui(!busy, |ui| {
            let hint = if creating { "Choose a master passphrase" } else { "Master passphrase" };
            input_bar(m, ui, local, hint, true)
        })
        .inner;
    let enter = ui.input(|i| i.key_pressed(Key::Enter));
    let mut submit = r.lost_focus() && enter;

    separator(ui, &m.pal);
    ui.add_space(14.0);
    if creating {
        note(ui, &m.pal, "No password file yet. A new encrypted one will be created at");
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            let path = tilde(&m.path.display().to_string());
            ui.label(RichText::new(path).font(FontId::monospace(12.5)));
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            let r2 = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut local.pass_confirm)
                    .password(true)
                    .hint_text("Repeat passphrase")
                    .margin(vec2(8.0, 6.0))
                    .desired_width(PANEL_W - 2.0 * GUTTER),
            );
            if r.lost_focus() && enter {
                r2.request_focus();
                submit = false;
            } else if r2.lost_focus() && enter {
                submit = true;
            }
        });
        ui.add_space(14.0);
    }

    if let Some(msg) = m.busy {
        spinner_line(ui, &m.pal, msg);
    } else {
        hints(ui, &m.pal, if creating { "enter create   esc close" } else { "enter unlock   esc close" });
    }

    if submit {
        out.push(Msg::Submit);
    }
}

fn view_list(m: &Model, ui: &mut egui::Ui, local: &mut Local, list: &[usize], out: &mut Vec<Msg>) {
    let p = m.pal;
    let ctx = ui.ctx().clone();
    let busy = m.busy.is_some();

    let r = input_bar(m, ui, local, "Search accounts", false);
    if r.changed() {
        local.selected = 0;
        out.push(Msg::SearchChanged);
    }
    separator(ui, &m.pal);

    if local.selected >= list.len() {
        local.selected = list.len().saturating_sub(1);
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
        if down && local.selected + 1 < list.len() {
            local.selected += 1;
            local.scroll_to_sel = true;
        }
        if up && local.selected > 0 {
            local.selected -= 1;
            local.scroll_to_sel = true;
        }
        if let Some(&i) = list.get(local.selected) {
            for (pressed, action) in [
                (enter, Msg::Pick(i)),
                (ctrl_u, Msg::CopyUser(i)),
                (ctrl_r, Msg::ToggleReveal(i)),
                (ctrl_e, Msg::BeginEdit(i)),
                (ctrl_d, Msg::AskDelete(i)),
            ] {
                if pressed {
                    out.push(action);
                }
            }
        }
        if ctrl_n {
            out.push(Msg::BeginNew);
        }
        if ctrl_l {
            out.push(Msg::Lock);
        }
    }

    // rows
    egui::ScrollArea::vertical()
        .max_height(m.rows_h(list))
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if list.is_empty() {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    if m.vault.as_ref().is_some_and(|v| v.entries.is_empty()) {
                        "No entries yet — ctrl+n to add one"
                    } else {
                        "No matching entries"
                    },
                    FontId::proportional(14.0),
                    p.weak,
                );
            }
            for (row, &i) in list.iter().enumerate() {
                let Some(e) = m.entry(i) else { continue };
                let sel = row == local.selected;
                let revealed = m.revealed == Some(i);
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), m.row_h(i)), Sense::click());
                if sel {
                    ui.painter().rect_filled(rect, 0.0, p.sel);
                    // 3px accent rule on the left edge
                    ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, p.accent);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 0.0, p.sel.gamma_multiply(0.55));
                }
                if sel && local.scroll_to_sel {
                    resp.scroll_to_me(None);
                    local.scroll_to_sel = false;
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
                    local.selected = row;
                    out.push(Msg::Pick(i));
                } else if resp.clicked() {
                    local.selected = row;
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
                            for (label, action, color) in [
                                ("delete", Msg::AskDelete(i), p.danger),
                                ("edit", Msg::BeginEdit(i), p.accent),
                                ("show", Msg::ToggleReveal(i), p.accent),
                                ("user", Msg::CopyUser(i), p.accent),
                                ("copy", Msg::Pick(i), p.accent),
                            ] {
                                if link(ui, label, color).clicked() {
                                    out.push(action);
                                }
                            }
                        },
                    );
                    ui.advance_cursor_after_rect(rect);
                }
            }
        });

    // footer: lowercase mono hints on the gutter, count on the right gutter
    let (frect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FOOT_H), Sense::hover());
    ui.painter().hline(frect.x_range(), frect.top() + 0.5, Stroke::new(1.0, p.border));
    let mono = FontId::monospace(11.0);
    let cy = frect.center().y + 0.5;
    let left = pos2(frect.left() + GUTTER, cy);
    let active_status =
        m.status.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(4)).map(|(m, _)| m.clone());
    if let Some(msg) = m.busy {
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
    let n = m.vault.as_ref().map_or(0, |v| v.entries.len());
    ui.painter().text(
        pos2(frect.right() - GUTTER, cy),
        Align2::RIGHT_CENTER,
        format!("{n} entries"),
        mono,
        p.weak,
    );
    if m.need_reload {
        let br = Rect::from_min_max(
            pos2(frect.right() - GUTTER - 210.0, frect.top()),
            pos2(frect.right() - GUTTER - 90.0, frect.bottom()),
        );
        ui.scope_builder(UiBuilder::new().max_rect(br).layout(Layout::right_to_left(Align::Center)), |ui| {
            if link(ui, "reload from disk", p.accent).clicked() {
                out.push(Msg::Reload);
            }
        });
        ui.advance_cursor_after_rect(frect);
    }
}

fn view_edit(m: &Model, ui: &mut egui::Ui, st: &mut EditState, out: &mut Vec<Msg>) {
    let p = m.pal;
    let busy = m.busy.is_some();
    let is_new = st.index.is_none();
    let mut regen = false;
    let w = PANEL_W - 2.0 * GUTTER;

    ui.add_space(22.0);
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.label(RichText::new(if is_new { "New entry" } else { "Edit entry" }).font(semibold(20.0)));
    });
    ui.add_space(16.0);
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
            if which == 2 && r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !busy {
                out.push(Msg::SaveEdit);
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
            out.push(Msg::SaveEdit);
        }
        if flat_button(ui, "Cancel", p.weak, p.bg, true).clicked() {
            out.push(Msg::Back);
        }
    });
}

fn view_confirm_delete(m: &Model, ui: &mut egui::Ui, i: usize, out: &mut Vec<Msg>) {
    let p = m.pal;
    let name = m.entry(i).map(|e| e.account().to_owned()).unwrap_or_default();
    ui.add_space(22.0);
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.label(RichText::new(format!("Delete “{name}”?")).font(semibold(20.0)));
    });
    ui.add_space(4.0);
    note(ui, &m.pal, "It is removed from the file immediately.");
    ui.add_space(16.0);
    if ui.input(|i| i.key_pressed(Key::Enter)) {
        out.push(Msg::ConfirmDelete);
    }
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.spacing_mut().item_spacing.x = 16.0;
        if flat_button(ui, "Delete", p.danger, p.bg, true).clicked() {
            out.push(Msg::ConfirmDelete);
        }
        if flat_button(ui, "Cancel", p.weak, p.bg, true).clicked() {
            out.push(Msg::Back);
        }
    });
}
