//! The look of jpass: layout constants, colours, fonts and the small
//! stateless widgets the screens are built from.

use eframe::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, FontData, FontFamily, FontId, Layout, Rect,
    RichText, Sense, Stroke, StrokeKind, UiBuilder, pos2, vec2,
};
use std::sync::Arc;

// Layout (logical px). The window is a bit larger than the visible panel so
// the drop shadow has room.
pub const MARGIN: f32 = 14.0;
pub const PANEL_W: f32 = 600.0;
pub const WIN_W: f32 = PANEL_W + MARGIN * 2.0;
pub const BAR_H: f32 = 48.0;
pub const ROW_H: f32 = 42.0;
pub const MAX_ROWS: usize = 7;
pub const FOOT_H: f32 = 26.0;
/// Left/right margin of everything inside the panel (icon, hints, links, forms).
pub const GUTTER: f32 = 20.0;
/// Text column: where the input text and the row text start, so they share one vertical axis.
pub const TEXT_X: f32 = 48.0;
/// Row icon box, its left edge on the gutter like the bar glyph.
pub const ICON: f32 = 20.0;
/// A revealed row gets a third line.
pub const REVEAL_EXTRA: f32 = 14.0;
pub const ERR_H: f32 = 40.0;
/// Breathing room under the last element of the lock screen and the forms.
pub const FORM_PAD: f32 = 16.0;

// Type sizes (logical px).
pub const BAR_FONT: f32 = 17.0;
pub const TITLE_FONT: f32 = 14.0;
pub const SUB_FONT: f32 = 11.5;
pub const SMALL_FONT: f32 = 12.5;
pub const HINT_FONT: f32 = 10.5;

// ───────────────────────────── theme ─────────────────────────────

#[derive(Clone, Copy)]
pub struct Pal {
    pub dark: bool,
    pub bg: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub sel: Color32,
    pub field: Color32,
    pub danger: Color32,
    pub ok: Color32,
    /// Hard, un-blurred offset shadow (Elm's "terminal" box).
    pub shadow: Color32,
}

impl Pal {
    pub fn new(dark: bool) -> Pal {
        if dark { Pal::DARK } else { Pal::LIGHT }
    }

    /// Acadia's cool gray scale.
    const DARK: Pal = Pal {
        dark: true,
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
    };

    /// elm-lang.org blue on white, ink from acadia.engineering.
    const LIGHT: Pal = Pal {
        dark: false,
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
    };
}

#[cfg(target_os = "macos")]
pub fn detect_dark() -> bool {
    let out = std::process::Command::new("defaults").args(["read", "-g", "AppleInterfaceStyle"]).output();
    out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("dark"))
}

/// Defaults to dark when no desktop answers.
#[cfg(not(target_os = "macos"))]
pub fn detect_dark() -> bool {
    kde_dark().or_else(gnome_dark).unwrap_or(true)
}

/// KDE: luminance of the window background colour.
#[cfg(not(target_os = "macos"))]
fn kde_dark() -> Option<bool> {
    let rgb = crate::desktop::kdeglobals("Colors:Window", "BackgroundNormal")?;
    let c: Vec<f32> = rgb.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (c.len() >= 3).then(|| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2] < 128.0)
}

/// GNOME & friends: the `color-scheme` setting.
#[cfg(not(target_os = "macos"))]
fn gnome_dark() -> Option<bool> {
    let scheme = crate::desktop::gsettings("org.gnome.desktop.interface", "color-scheme")?.to_lowercase();
    if scheme.contains("dark") {
        return Some(true);
    }
    (scheme.contains("light") || scheme.contains("default")).then_some(false)
}

pub fn apply_theme(ctx: &egui::Context, p: &Pal) {
    let mut v = visuals(p);
    square_corners(&mut v);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(10.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
    });
}

/// egui's stock widgets in the palette's colours: 1px borders, accent on hover and focus.
fn visuals(p: &Pal) -> egui::Visuals {
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
    v
}

fn square_corners(v: &mut egui::Visuals) {
    let w = &mut v.widgets;
    for w in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.noninteractive, &mut w.open] {
        w.corner_radius = CornerRadius::ZERO;
    }
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
}

/// IBM Plex, as used on elm-lang.org and acadia.engineering.
pub fn install_fonts(ctx: &egui::Context) {
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

const PLEX_SANS: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");
const PLEX_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf");
const PLEX_MONO: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

// ───────────────────────────── the input bar ─────────────────────────────

/// The 64px input bar: glyph on the left gutter, text on the shared text column.
/// Leaves the layout cursor exactly at the bottom of the bar.
/// `lock_state`: `None` = no indicator, `Some(true)` = vault locked (closed lock),
/// `Some(false)` = vault unlocked (open lock).
pub fn search_bar(
    ui: &mut egui::Ui,
    p: &Pal,
    text: &mut String,
    focus: &mut bool,
    hint: &str,
    locked: bool,
    lock_state: Option<bool>,
) -> egui::Response {
    let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
    let cy = rect.center().y + 0.5;
    bar_glyph(ui.painter(), p, rect.left() + GUTTER, cy, locked);
    let r = bar_field(ui, p, bar_field_rect(rect, lock_state.is_some()), text, hint, locked);
    ui.advance_cursor_after_rect(rect);
    ui.spacing_mut().item_spacing.y = gap;
    if let Some(is_locked) = lock_state {
        lock_indicator(ui.painter(), p, rect.right() - GUTTER, cy, is_locked);
    }
    if std::mem::take(focus) {
        r.request_focus();
    }
    r
}

/// Measured: with this box the x-height band of the text and the glyph share one centre.
fn bar_field_rect(bar: Rect, with_indicator: bool) -> Rect {
    let indicator_w = if with_indicator { 26.0 } else { 0.0 };
    Rect::from_min_max(
        pos2(bar.left() + TEXT_X, bar.top() + 6.0),
        pos2(bar.right() - GUTTER - indicator_w, bar.bottom() - 11.0),
    )
}

/// Left glyph of the bar: a padlock for the passphrase, else a magnifier.
fn bar_glyph(painter: &egui::Painter, p: &Pal, left: f32, cy: f32, locked: bool) {
    let st = Stroke::new(1.5, p.text);
    if locked {
        padlock(painter, Rect::from_min_size(pos2(left, cy - 2.5), vec2(13.0, 9.5)), 4.0, false, st);
    } else {
        let c = pos2(left + 6.3, cy - 1.5);
        painter.circle_stroke(c, 5.5, st);
        painter.line_segment([c + vec2(4.1, 4.1), c + vec2(8.2, 8.2)], st);
    }
}

fn bar_field(ui: &mut egui::Ui, p: &Pal, field: Rect, text: &mut String, hint: &str, password: bool) -> egui::Response {
    let edit = egui::TextEdit::singleline(text)
        .font(FontId::proportional(BAR_FONT))
        .frame(egui::Frame::NONE)
        .password(password)
        .hint_text(RichText::new(hint).font(FontId::proportional(BAR_FONT)).color(p.weak))
        .desired_width(field.width())
        .margin(vec2(0.0, 4.0));
    let layout = Layout::left_to_right(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(field).layout(layout), |ui| ui.add(edit)).inner
}

/// Vault state on the right of the bar: closed and grey, or open and accented.
fn lock_indicator(painter: &egui::Painter, p: &Pal, right: f32, cy: f32, locked: bool) {
    let color = if locked { p.weak } else { p.accent };
    let body = Rect::from_min_size(pos2(right - 10.0, cy - 3.5), vec2(11.0, 8.0));
    padlock(painter, body, 3.5, !locked, Stroke::new(1.3, color));
}

// ───────────────────────────── widgets ─────────────────────────────

/// 1px rule directly under the bar.
pub fn separator(ui: &mut egui::Ui, p: &Pal) {
    let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, p.border));
    ui.spacing_mut().item_spacing.y = gap;
}

/// A small grey line of text on the gutter.
pub fn note(ui: &mut egui::Ui, p: &Pal, text: impl Into<String>) {
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.label(RichText::new(text.into()).color(p.weak).size(SMALL_FONT));
    });
}

/// Lowercase monospace key hints, same style as the list footer.
pub fn hints(ui: &mut egui::Ui, p: &Pal, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.label(RichText::new(text).font(FontId::monospace(HINT_FONT)).color(p.weak));
    });
}

/// A spinner followed by a grey message, on the gutter.
pub fn spinner_line(ui: &mut egui::Ui, p: &Pal, msg: &str) {
    ui.horizontal(|ui| {
        ui.add_space(GUTTER);
        ui.spinner();
        ui.label(RichText::new(msg).color(p.weak).size(SMALL_FONT));
    });
}

/// A fixed-height strip with up to two lines of error text and "dismiss" on
/// the right gutter. Returns true when "dismiss" was clicked.
pub fn error_banner(ui: &mut egui::Ui, p: &Pal, msg: &str) -> bool {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ERR_H), Sense::hover());
    let galley = ui.painter().layout(msg.to_owned(), FontId::proportional(SMALL_FONT), p.danger, r.width() - 2.0 * GUTTER - 66.0);
    let pos = pos2(r.left() + GUTTER, r.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, p.danger);
    let dm = Rect::from_min_max(pos2(r.right() - GUTTER - 60.0, r.top()), pos2(r.right() - GUTTER, r.bottom()));
    let mut dismissed = false;
    ui.scope_builder(UiBuilder::new().max_rect(dm).layout(Layout::right_to_left(Align::Center)), |ui| {
        dismissed = link(ui, "dismiss", p.weak).clicked();
    });
    ui.advance_cursor_after_rect(r);
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.border));
    dismissed
}

/// Plain text link in the accent colour; underlined on hover (elm-lang.org).
pub fn link(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(SMALL_FONT), color);
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
pub fn flat_button(ui: &mut egui::Ui, label: &str, color: Color32, bg: Color32, enabled: bool) -> egui::Response {
    let font = FontId::proportional(13.5);
    let size = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), color).size() + vec2(24.0, 10.0);
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

// ───────────────────────────── glyphs ─────────────────────────────

/// Padlock: a shackle arc of radius `r` over `body`; `open` lifts it off to the right.
pub fn padlock(painter: &egui::Painter, body: Rect, r: f32, open: bool, stroke: Stroke) {
    painter.rect_stroke(body, 0.0, stroke, StrokeKind::Inside);
    let shackle = if open { pos2(body.center().x + 2.5, body.top() - 2.0) } else { pos2(body.center().x, body.top()) };
    painter.circle_stroke(shackle, r, stroke);
}

/// Stand-in app icon: a 2x2 grid of 6px squares around `center`.
pub fn grid_glyph(painter: &egui::Painter, center: egui::Pos2, stroke: Stroke) {
    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let square = Rect::from_center_size(center + vec2(dx * 4.5, dy * 4.5), vec2(6.0, 6.0));
        painter.rect_stroke(square, 1.0, stroke, StrokeKind::Inside);
    }
}

/// Replace the home directory prefix with `~`.
pub fn tilde(path: &str) -> String {
    match dirs::home_dir().and_then(|h| h.to_str().map(str::to_owned)) {
        Some(h) => path.replacen(&h, "~", 1),
        None => path.to_owned(),
    }
}
