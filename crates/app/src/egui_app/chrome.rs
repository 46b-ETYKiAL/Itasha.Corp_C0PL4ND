//! The C0PL4ND chrome: a frameless titlebar (two-tone wordmark + tab strip +
//! caption buttons), a settings gear, and a bottom status bar. Window controls
//! use `egui::ViewportCommand` (no Win32) per recon dossier §3.2.
//!
//! The titlebar mirrors the sibling SCR1B3 editor's frameless titlebar for a
//! same-product-family read: a `horizontal_centered` row with the left content
//! (wordmark + tabs + split buttons) first, then the caption cluster
//! (settings/minimize/maximize/close) pinned to the window's right edge. The
//! cluster is placed at absolute rects anchored to `ctx().screen_rect()` — the
//! only reliable bounded right edge in this non-justified row (the ui's own
//! `clip_rect()`/`max_rect()` right edge is unbounded in the render frame). The
//! buttons use Phosphor glyphs via `ui.button` so they self-size and never fall
//! back to tofu.

use c0pl4nd_core::term::MouseMode;
use egui::{RichText, Sense};
use egui_phosphor::thin as icon;

use super::theme::{brand, ChromeColors};
use super::C0pl4ndApp;

/// Outcome of one chrome frame — the actions the user requested via the chrome
/// widgets. The host applies them after the panel closure returns so that the
/// grid/tree mutation does not happen mid-borrow.
#[derive(Debug, Default, Clone)]
pub struct ChromeActions {
    /// User clicked a tab; switch focus to this pane.
    pub focus_tab: Option<super::grid::PaneId>,
    /// User clicked a tab's close (×); close this pane.
    pub close_tab: Option<super::grid::PaneId>,
    /// User clicked a tab's pin; toggle this pane's pinned state.
    pub pin_tab: Option<super::grid::PaneId>,
    /// User clicked the "+" new-terminal button; open a new pane (the host picks
    /// the split direction to keep the grid balanced).
    pub new_terminal: bool,
    /// User picked a shell from the top-bar ▾ switcher; index into
    /// [`super::C0pl4ndApp::shell_profiles`]. The host opens a new terminal with
    /// that shell and makes it the active profile for the plain "+" button.
    pub open_shell: Option<usize>,
    /// User toggled the settings window.
    pub toggle_settings: bool,
    /// User clicked a caption button (minimize / maximize / close). Routed
    /// through the action struct (instead of sending the `ViewportCommand`
    /// inline) so `frame_tick` is the single place that issues the real OS
    /// command AND records it for the interaction tests to observe — a click on
    /// the real button thus has an assertable outcome without a window.
    pub window_cmd: Option<super::WindowCmd>,
}

impl C0pl4ndApp {
    /// Paint the titlebar (wordmark + tab strip + caption controls). Returns the
    /// actions the host should apply this frame. `colors` carries the
    /// theme-derived chrome palette so the tab text / caption glyphs / accents
    /// follow the active terminal theme (the two-tone C0PL4ND wordmark keeps its
    /// fixed brand accent — the brand identity).
    pub(super) fn titlebar_and_tabs(
        &self,
        ui: &mut egui::Ui,
        colors: ChromeColors,
    ) -> ChromeActions {
        let mut actions = ChromeActions::default();
        // One left→right row: the LEFT content (wordmark + tabs + split buttons)
        // flows normally; the caption cluster is then placed at absolute rects
        // pinned to the window's right edge (see the cluster block below).
        ui.horizontal_centered(|ui| {
            // two-tone C0PL4ND wordmark + drag/double-click caption region.
            let mut job = egui::text::LayoutJob::default();
            let fmt = |color| egui::text::TextFormat {
                color,
                font_id: egui::FontId::proportional(16.0),
                ..Default::default()
            };
            job.append("C0PL", 0.0, fmt(brand::PURPLE));
            job.append("4ND", 0.0, fmt(brand::GREEN));
            let title_resp = ui.add(egui::Label::new(job).sense(Sense::click_and_drag()));
            if title_resp.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if title_resp.double_clicked() {
                actions.window_cmd = Some(super::WindowCmd::ToggleMaximize);
            }

            ui.separator();

            // Tab strip: one tab per pane, SCR1B3-style — each tab is
            // [title] [pin] [×]. Pinned tabs sort first and carry a violet pin;
            // their × is hidden (unpin to close) so they can't be shut by
            // accident. Clicking the title focuses the pane; × closes it.
            let mut tabs = self.pane_titles();
            // Stable sort: pinned first, original visual order preserved within
            // each group (`sort_by_key` is stable).
            tabs.sort_by_key(|(pid, _)| !self.pinned.contains(pid));
            for (pane_id, title) in tabs {
                let selected = pane_id == self.focused_pane;
                let is_pinned = self.pinned.contains(&pane_id);
                // Per-tab accessible labels. The pin/× glyph buttons AND the tab
                // itself would otherwise expose a NON-unique name (the title),
                // and two shells in the same directory routinely set the SAME OSC
                // title — ambiguous for screen readers AND for `get_by_label`
                // tests. Anchor every label on the unique `pane {id}` so each tab
                // is distinguishable even when titles collide. The VISIBLE tab
                // text stays the bare title; only the accessible name carries the
                // id suffix.
                let a11y = Self::tab_a11y_label(pane_id, &title);
                let pin_label = format!("{} {a11y}", if is_pinned { "unpin" } else { "pin" });
                let close_label = format!("close {a11y}");
                ui.scope(|ui| {
                    // Tight spacing INSIDE a tab so title/pin/× read as one unit.
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let label = RichText::new(&title).color(if selected {
                        colors.accent
                    } else {
                        colors.fg
                    });
                    let tab = ui.selectable_label(selected, label);
                    // Override the accessible name with the UNIQUE label so the
                    // a11y tree never has two same-named tab nodes (the visible
                    // text is unchanged — still just the title).
                    tab.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::SelectableLabel, true, &a11y)
                    });
                    if tab.clicked() {
                        actions.focus_tab = Some(pane_id);
                    }
                    // Pinned → SOLID violet pin (Fill family); unpinned → thin
                    // muted pin. The fill glyph makes "pinned" read at a glance.
                    let pin_text = if is_pinned {
                        RichText::new(egui_phosphor::fill::PUSH_PIN)
                            .family(egui::FontFamily::Name("phosphor-fill".into()))
                            .size(13.0)
                            .color(brand::PURPLE)
                    } else {
                        RichText::new(icon::PUSH_PIN).size(13.0).color(colors.muted)
                    };
                    let pin = ui
                        .add(egui::Button::new(pin_text).frame(false))
                        .on_hover_text(&pin_label);
                    pin.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &pin_label)
                    });
                    if pin.clicked() {
                        actions.pin_tab = Some(pane_id);
                    }
                    if !is_pinned {
                        let close = ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon::X).size(13.0).color(colors.muted),
                                )
                                .frame(false),
                            )
                            .on_hover_text(&close_label);
                        close.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &close_label)
                        });
                        if close.clicked() {
                            actions.close_tab = Some(pane_id);
                        }
                    }
                });
                ui.separator();
            }

            // Single "+" new-terminal button: opens a new pane and lets the host
            // expand the grid logically (it splits the focused pane along its
            // longer axis, keeping panes balanced — no manual direction choice).
            // It runs the active shell profile (set via the ▾ switcher below).
            let new_term = ui
                .button(RichText::new(icon::PLUS).size(16.0))
                .on_hover_text(format!("new terminal ({})", self.active_shell_label()));
            new_term.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "new terminal")
            });
            if new_term.clicked() {
                actions.new_terminal = true;
            }

            // Shell switcher (▾): lists the shells detected on this machine.
            // Picking one opens a new terminal running it AND makes it the active
            // profile for the plain "+" button — the Windows-Terminal "+ ▾"
            // profile pattern. This is the user's "run things other than
            // PowerShell — an easy switch in the top bar" affordance.
            let menu = ui.menu_button(
                RichText::new(format!("{} ▾", icon::TERMINAL_WINDOW)).size(13.0),
                |ui| {
                    ui.label(RichText::new("Open a new terminal with…").weak().small());
                    ui.separator();
                    let active = self.active_shell_label().to_owned();
                    for (i, profile) in self.shell_profiles().iter().enumerate() {
                        let mut label = profile.label.clone();
                        if profile.label == active {
                            label.push_str("  ✓");
                        }
                        let item = ui.button(&label);
                        item.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                format!("open shell {}", profile.label),
                            )
                        });
                        if item.clicked() {
                            actions.open_shell = Some(i);
                        }
                    }
                },
            );
            menu.response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "shell menu")
            });
            menu.response
                .on_hover_text("Choose which shell new terminals run");

            // ---- right-pinned caption cluster ----
            // Placed at ABSOLUTE rects via `ui.put`. Every layout-flow attempt
            // (`right_to_left`, `Sides`, `allocate_ui_with_layout`, an
            // `available_width()` spacer) AND every right-edge taken from the
            // ui's own rects left the right side EMPTY: in this non-justified
            // `horizontal_centered` the ui's `clip_rect()`/`max_rect()` right edge
            // is UNBOUNDED in the render frame (`rect_filled(clip_rect)` paints
            // the visible width, but `clip_rect().right()` is ~f32::MAX), so a
            // right-anchored x landed off-screen. The window's `screen_rect()` is
            // the only reliable bounded right edge; `min_rect()` (the content laid
            // out so far) gives the true row Y. Reads left→right ⚙ — ▢ ✕.
            let screen = ui.ctx().content_rect();
            let row = ui.min_rect();
            let bw = 42.0_f32;
            let bh = 28.0_f32;
            let cy = row.center().y;
            let right_edge = screen.right() - 8.0; // window edge minus panel inset
            let specs: [(&str, &str, super::WindowCmd, bool); 4] = [
                (icon::X, "close", super::WindowCmd::Close, false),
                (
                    icon::SQUARE,
                    "maximize",
                    super::WindowCmd::ToggleMaximize,
                    false,
                ),
                (icon::MINUS, "minimize", super::WindowCmd::Minimize, false),
                (icon::GEAR, "settings", super::WindowCmd::Close, true), // gear → settings
            ];
            let mut right_x = right_edge;
            for (glyph, hover, cmd, is_gear) in specs {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(right_x - bw, cy - bh / 2.0),
                    egui::pos2(right_x, cy + bh / 2.0),
                );
                let resp = ui
                    .put(
                        rect,
                        egui::Button::new(RichText::new(glyph).size(16.0).color(colors.muted)),
                    )
                    .on_hover_text(hover);
                // Accessible label (for screen readers AND the `get_by_label`
                // interaction tests) — the visible content is a glyph, so the
                // semantic name must be set explicitly.
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, hover)
                });
                if resp.clicked() {
                    if is_gear {
                        actions.toggle_settings = true;
                    } else {
                        actions.window_cmd = Some(cmd);
                    }
                }
                right_x -= bw + 2.0;
            }
        });
        actions
    }

    /// Paint the bottom status bar — pane count + a theme-tinted hint. `colors`
    /// carries the theme-derived palette so the bar follows the active theme.
    pub(super) fn status_bar(&self, ui: &mut egui::Ui, colors: ChromeColors) {
        ui.horizontal(|ui| {
            let panes = super::grid::count_panes(&self.grid_tree);
            ui.label(
                RichText::new(format!("{panes}/{} panes", super::grid::MAX_PANES))
                    .color(colors.accent),
            );
            ui.separator();
            ui.label(
                RichText::new("C0PL4ND — local-first terminal")
                    .color(colors.fg)
                    .weak(),
            );
            ui.separator();
            ui.label(
                RichText::new("Ctrl+Shift+P: commands")
                    .color(colors.fg)
                    .weak(),
            );
            // Mouse-reporting badge: when the FOCUSED pane's TUI has grabbed the
            // mouse (DEC ?1000/?1002/?1003), show a small badge so the user can
            // see why their clicks/scroll go to the app instead of the terminal.
            // Hidden entirely when reporting is Off (the common case).
            if let Some(term) = self.terms.get(&self.focused_pane) {
                let mode = term.mouse_mode();
                if let Some(label) = mouse_mode_badge_label(mode) {
                    ui.separator();
                    ui.label(
                        RichText::new(format!("{} {label}", icon::MOUSE_SIMPLE))
                            .color(colors.accent),
                    )
                    .on_hover_text(
                        "The focused application has enabled mouse reporting \
                         (clicks and scroll are sent to the program).",
                    );
                }
            }
            // Exit-code indicator: the FOCUSED pane's last finished command's
            // OSC 133 `D` exit code. A green check for success (0), an X plus
            // the code for a failure. Hidden entirely when no command has
            // finished (a bare shell with no prompt integration never emits a
            // `D` mark — the common case).
            if let Some(term) = self.terms.get(&self.focused_pane) {
                if let Some(indicator) = exit_code_indicator(term.last_command_exit_code()) {
                    ui.separator();
                    // Success uses the theme/brand green live-accent; a failure
                    // uses the muted foreground (Akira-red #ff0040 is reserved
                    // for alarms, not routine non-zero command exits).
                    let color = if indicator.is_failure {
                        colors.muted
                    } else {
                        brand::GREEN
                    };
                    ui.label(RichText::new(indicator.text).color(color))
                        .on_hover_text(indicator.hover);
                }
            }
            if let Some(toast) = &self.toast {
                ui.separator();
                ui.label(RichText::new(toast).color(colors.accent));
            }
        });
    }
}

/// A rendered exit-code status-bar indicator: the glyph+code text, an
/// accessible hover label, and whether it represents a failed command (so the
/// caller picks the colour). Kept as a plain struct returned by a free function
/// so the indicator-selection logic is unit-testable without an egui `Ui`.
struct ExitCodeIndicator {
    /// The status-bar text — a Phosphor glyph plus, for failures, the code.
    text: String,
    /// Accessible hover/description label (mirrors the mouse-mode badge).
    hover: &'static str,
    /// `true` for a non-zero exit code (drives the failure colour).
    is_failure: bool,
}

/// Build the status-bar [`ExitCodeIndicator`] for a pane's
/// [`last_command_exit_code`](super::pane_term::PaneTerm::last_command_exit_code)
/// value, or `None` when no command has finished yet (no indicator shown).
///
/// - outer `None` → `None` (no finished command; the status bar shows nothing);
/// - `Some(Some(0))` → green check, "success" (`is_failure = false`);
/// - `Some(Some(code))` for `code != 0` → X + the code, "failed"
///   (`is_failure = true`);
/// - `Some(None)` → a check glyph with a "finished (no exit code reported)"
///   label, treated as non-failure so it does not alarm.
fn exit_code_indicator(exit: Option<Option<i32>>) -> Option<ExitCodeIndicator> {
    let code = exit?;
    Some(match code {
        Some(0) => ExitCodeIndicator {
            text: icon::CHECK_CIRCLE.to_string(),
            hover: "Last command succeeded (exit code 0).",
            is_failure: false,
        },
        Some(code) => ExitCodeIndicator {
            text: format!("{} {code}", icon::X_CIRCLE),
            hover: "Last command failed (non-zero exit code).",
            is_failure: true,
        },
        None => ExitCodeIndicator {
            text: icon::CHECK_CIRCLE.to_string(),
            hover: "Last command finished (no exit code reported).",
            is_failure: false,
        },
    })
}

/// The status-bar badge label for a mouse-reporting mode, or `None` when mouse
/// reporting is [`MouseMode::Off`] (no badge shown). Kept as a free function so
/// the badge-visibility logic is unit-testable without an egui `Ui`.
fn mouse_mode_badge_label(mode: MouseMode) -> Option<&'static str> {
    match mode {
        MouseMode::Off => None,
        MouseMode::Normal => Some("MOUSE"),
        MouseMode::ButtonEvent => Some("MOUSE: BTN"),
        MouseMode::AnyEvent => Some("MOUSE: ANY"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_mode_badge_hidden_when_off() {
        assert_eq!(mouse_mode_badge_label(MouseMode::Off), None);
    }

    #[test]
    fn mouse_mode_badge_shown_when_reporting() {
        assert_eq!(mouse_mode_badge_label(MouseMode::Normal), Some("MOUSE"));
        assert_eq!(
            mouse_mode_badge_label(MouseMode::ButtonEvent),
            Some("MOUSE: BTN")
        );
        assert_eq!(
            mouse_mode_badge_label(MouseMode::AnyEvent),
            Some("MOUSE: ANY")
        );
    }

    #[test]
    fn exit_code_indicator_hidden_when_no_finished_command() {
        assert!(
            exit_code_indicator(None).is_none(),
            "no finished command must show no indicator"
        );
    }

    #[test]
    fn exit_code_indicator_success_is_not_a_failure() {
        let ind = exit_code_indicator(Some(Some(0))).expect("success must show an indicator");
        assert!(!ind.is_failure, "exit code 0 is a success, not a failure");
        assert_eq!(
            ind.text,
            icon::CHECK_CIRCLE.to_string(),
            "success shows a bare check glyph (no code)"
        );
    }

    #[test]
    fn exit_code_indicator_failure_shows_code() {
        let ind = exit_code_indicator(Some(Some(127))).expect("failure must show an indicator");
        assert!(ind.is_failure, "non-zero exit code is a failure");
        assert_eq!(
            ind.text,
            format!("{} 127", icon::X_CIRCLE),
            "failure shows the X glyph plus the exit code"
        );
    }

    #[test]
    fn exit_code_indicator_missing_code_is_neutral() {
        // A finished command with no shell-reported code (`OSC 133 ; D`) shows
        // a non-alarming indicator rather than being hidden.
        let ind = exit_code_indicator(Some(None)).expect("finished command must show an indicator");
        assert!(
            !ind.is_failure,
            "an absent exit code must not be treated as a failure"
        );
    }
}
