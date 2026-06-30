//! `C0pl4ndApp` W1TN3SS crash-consent + report-issue dialogs.
//!
//! The per-launch crash-consent overlay (editable preview, equal-weight Send /
//! Don't-send, remembered Always/Never) and the report-an-issue form, both gated
//! through the SDK consent path. Grouped out of the C0pl4ndApp god-impl; behaviour
//! unchanged. Nothing transmits without an explicit click + consent token.

use eframe::egui;

impl super::C0pl4ndApp {
    /// Render the W1TN3SS per-launch crash-consent dialog, if a spooled crash
    /// report is pending (drained by [`Self::drain_crash_spool`] on launch when
    /// the crash stream is `AskEachTime`). Presents ONE report at a time with an
    /// EDITABLE preview + equal-weight Send / Don't-send; persists a remembered
    /// "Always"/"Never" choice into the config. A no-op when nothing is pending
    /// (the opted-out / opted-in-but-empty case) — so the default experience is
    /// untouched. Nothing transmits until the user clicks Send (which mints the
    /// consent token inside the SDK-gated `consent_and_send`).
    pub(crate) fn render_crash_consent(&mut self, ctx: &egui::Context) {
        if !self.crash_consent.has_pending() {
            return;
        }
        let mut do_send = false;
        let mut do_decline = false;
        egui::Window::new("Send this crash report?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    "C0PL4ND captured a crash. You can review and edit the report \
                     below before deciding — nothing is sent unless you choose Send.",
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(self.crash_consent.edited_text_mut())
                                .desired_rows(8)
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace),
                        );
                    });
                ui.add_space(8.0);
                ui.label("Remember my choice for future crashes:");
                let remember = self.crash_consent.remember_mut();
                ui.horizontal(|ui| {
                    ui.radio_value(
                        remember,
                        Some(crate::reporting::RememberChoice::JustThisTime),
                        "Just this time",
                    );
                    ui.radio_value(
                        remember,
                        Some(crate::reporting::RememberChoice::Always),
                        "Always send",
                    );
                    ui.radio_value(
                        remember,
                        Some(crate::reporting::RememberChoice::Never),
                        "Never",
                    );
                });
                ui.add_space(8.0);
                // Equal-weight Send / Don't-send — no dark-pattern asymmetry.
                ui.horizontal(|ui| {
                    if ui.button("Send report").clicked() {
                        do_send = true;
                    }
                    if ui.button("Don't send").clicked() {
                        do_decline = true;
                    }
                });
            });
        // Persist a remembered Always/Never BEFORE advancing (so the choice
        // applies from the next launch). JustThisTime persists nothing.
        let remembered = *self.crash_consent.remember_mut();
        if do_send {
            if let Some(choice) = remembered {
                self.apply_remember_choice(choice);
            }
            let _ = self.crash_consent.consent_and_send();
        } else if do_decline {
            if let Some(choice) = remembered {
                self.apply_remember_choice(choice);
            }
            self.crash_consent.decline_and_discard();
        }
    }

    /// Persist a remembered crash-consent choice into the config (graduating the
    /// crash stream to `Always` or `Off`), then save the config so the next
    /// launch honours it. `JustThisTime` persists nothing. Best-effort save.
    fn apply_remember_choice(&mut self, choice: crate::reporting::RememberChoice) {
        if let Some(mode) = choice.persisted_mode() {
            self.config.reporting.streams.crash_reports = mode;
            if let Some(path) = c0pl4nd_core::Config::default_path() {
                if let Err(e) = self.config.save_to(&path) {
                    self.toast = Some(crate::user_error::config_save_failed(
                        e,
                        "Your reporting choice",
                    ));
                }
            }
        }
    }

    /// Render the W1TN3SS manual "Report an issue" dialog, opened from the
    /// titlebar script menu ([`crate::issue_intake::IssueIntakeState::open_fresh`]
    /// sets `open`). User-initiated only: renders only when `issue_intake.open`.
    /// Builds a prefilled GitHub Issue-Form deep link (or a clipboard / mailto
    /// fallback); diagnostics are OFF unless the user ticks them; the body is
    /// previewable + editable before anything leaves. Nothing transmits — it
    /// hands a URL to the browser / a body to the clipboard on an explicit click.
    pub(crate) fn render_report_issue(&mut self, ctx: &egui::Context) {
        if !self.issue_intake.open {
            return;
        }
        let repo = self.config.reporting.issue_intake.repo.clone();
        let alias = self.config.reporting.issue_intake.mailto_alias.clone();
        let renderer = crate::issue_intake::RENDERER;

        // Decisions deferred past the closure borrow (like paste_confirm_window).
        let mut do_open = false;
        let mut do_mailto = false;
        let mut do_close = false;
        egui::Window::new("Report an issue")
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    "Open a prefilled GitHub issue. Nothing is sent automatically — \
                     your browser opens the issue form for you to review and submit.",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Kind:");
                    for kind in crate::issue_intake::IssueKind::ALL {
                        ui.radio_value(&mut self.issue_intake.kind, kind, kind.display());
                    }
                });
                ui.add_space(6.0);
                ui.label("Describe the issue:");
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.issue_intake.description)
                                .desired_rows(5)
                                .desired_width(f32::INFINITY),
                        );
                    });
                ui.add_space(6.0);
                ui.checkbox(
                    &mut self.issue_intake.include_diagnostics,
                    "Include non-identifying diagnostics (app version, OS, renderer)",
                );
                ui.add_space(6.0);
                // Faithful preview of EXACTLY what will be sent.
                let preview = self.issue_intake.preview_body(renderer);
                ui.label("Preview:");
                egui::ScrollArea::vertical()
                    .id_salt("issue_preview")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&preview).monospace()).wrap());
                    });
                ui.add_space(8.0);
                if !self.issue_intake.fits_url_length(&repo, renderer) {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "This report is long — \"Open on GitHub\" will copy it \
                                 to your clipboard to paste into a blank issue instead.",
                            )
                            .weak()
                            .small(),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                }
                // Show the last outcome (status line).
                if let Some(outcome) = &self.issue_intake.last_outcome {
                    let msg = match outcome {
                        crate::issue_intake::IntakeOutcome::OpenedDeepLink => {
                            "Opened the issue form in your browser."
                        }
                        crate::issue_intake::IntakeOutcome::CopiedToClipboard => {
                            "Copied the report to your clipboard — paste it into a new issue."
                        }
                        crate::issue_intake::IntakeOutcome::OpenedMailto => {
                            "Opened your mail client."
                        }
                        crate::issue_intake::IntakeOutcome::Failed(_) => {
                            "That didn't work. You can copy the report to your clipboard and \
                             paste it into a new GitHub issue instead."
                        }
                    };
                    ui.add(egui::Label::new(egui::RichText::new(msg).small()).wrap());
                    ui.add_space(4.0);
                }
                ui.horizontal(|ui| {
                    if ui.button("Open on GitHub").clicked() {
                        do_open = true;
                    }
                    if ui.button("Email instead").clicked() {
                        do_mailto = true;
                    }
                    if ui.button("Close").clicked() {
                        do_close = true;
                    }
                });
            });

        if do_open {
            let req = self.issue_intake.request(&repo, renderer);
            let outcome = match crate::issue_intake::open_or_copy(&req) {
                crate::issue_intake::IntakeAction::Opened(o) => o,
                crate::issue_intake::IntakeAction::CopyToClipboard(body) => {
                    // C0PL4ND has no `arboard`: copy via egui's clipboard here.
                    ctx.copy_text(body);
                    crate::issue_intake::IntakeOutcome::CopiedToClipboard
                }
            };
            crate::issue_intake::log_outcome(&outcome);
            self.issue_intake.last_outcome = Some(outcome);
        } else if do_mailto {
            let req = self.issue_intake.request(&repo, renderer);
            let outcome = crate::issue_intake::open_mailto(&alias, &req.title, &req.body);
            crate::issue_intake::log_outcome(&outcome);
            self.issue_intake.last_outcome = Some(outcome);
        } else if do_close {
            self.issue_intake.open = false;
        }
    }
}
