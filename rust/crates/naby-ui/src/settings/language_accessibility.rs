//! Interface language, text size and contrast (C# `SettingsLanguageAccessibility.cs`). These
//! three are edited as a draft and written by "Save language and accessibility"; reduce motion
//! is a normal live switch.

use super::SettingsView;
use naby_core::i18n::languages;
use naby_render::{Fill, Ui, Weight};

const SIZES: [f64; 4] = [1.0, 1.1, 1.25, 1.5];
const SIZE_LABELS: [&str; 4] = ["100%", "110%", "125%", "150%"];

/// The unsaved choices of the section.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LanguageDraft {
    pub language: usize,
    pub size: usize,
    pub contrast: bool,
}

impl LanguageDraft {
    pub fn of(s: &naby_core::settings::Settings) -> LanguageDraft {
        LanguageDraft {
            language: languages()
                .iter()
                .position(|l| l.code == s.ui_language)
                .unwrap_or(0),
            size: SIZES
                .iter()
                .position(|v| *v >= s.accessibility_text_scale)
                .unwrap_or(SIZES.len() - 1),
            contrast: s.accessibility_high_contrast,
        }
    }
}

impl SettingsView {
    pub(super) fn language_and_accessibility_settings<'a>(&mut self, ui: &mut Ui<'a>) {
        self.section(ui, "Language & accessibility", |this, ui| {
            let mut draft = this
                .lang_draft
                .clone()
                .unwrap_or_else(|| LanguageDraft::of(&this.s));
            let th = ui.theme().clone();
            this.muted(ui, "Interface language", 12.0);
            ui.gap(4.0);
            let names: Vec<&str> = languages().iter().map(|l| l.name).collect();
            let resp = ui.combo(&mut draft.language, &names);
            this.record("Interface language", resp.rect);
            this.hand |= resp.hovered;
            ui.gap(10.0);
            this.muted(
                ui,
                "English is used when a translation is unavailable. Your content stays in its original language.",
                11.0,
            );
            ui.gap(10.0);
            this.muted(ui, "Text size", 12.0);
            ui.gap(4.0);
            let resp = ui.combo(&mut draft.size, &SIZE_LABELS);
            this.record("Text size", resp.rect);
            this.hand |= resp.hovered;
            ui.gap(10.0);
            // High contrast: label and switch.
            let rect = ui.allocate(None, 40.0);
            let r = ui.hit_area(rect, true);
            this.record("High contrast", rect);
            this.hand |= r.hovered;
            if r.clicked {
                draft.contrast = !draft.contrast;
            }
            let st = ui.text_style(13.0, Weight::Regular, th.text);
            let lh = ui.line_height(&st);
            let label = this.tr("High contrast").to_owned();
            ui.draw_text(rect.x, rect.y + (rect.h - lh) / 2.0, &label, &st, Some(rect.w - 58.0));
            let track = naby_render::Rect::new(rect.right() - 40.0, rect.y + 10.0, 34.0, 20.0);
            let c = ui.canvas();
            c.fill_rrect(track, 10.0, if draft.contrast { th.checked_fill } else { th.track_fill });
            let kx = if draft.contrast { track.right() - 17.0 } else { track.x + 3.0 };
            c.fill_circle(
                kx + 7.0,
                track.y + 10.0,
                7.0,
                &Fill::Solid(if draft.contrast { th.checked_text } else { th.switch_knob }),
            );
            ui.paint_focus_ring(&r, 8.0);
            this.muted(
                ui,
                "Windows high contrast is always respected. Larger text may require scrolling in compact widgets.",
                11.0,
            );
            ui.gap(6.0);
            let reduce = this.s.reduce_motion;
            this.check(ui, "Reduce motion", reduce, |s, v| s.reduce_motion = v);
            this.muted(ui, "Motion also follows the Windows animation setting.", 11.0);
            let save = this.primary_button(ui, "Save language and accessibility");
            let note = this.restart_note.clone().unwrap_or_else(|| {
                "Save these preferences, then quit and reopen NabyScape to apply them.".to_owned()
            });
            ui.gap(8.0);
            let shown = this.tr(&note).to_owned();
            this.muted_final(ui, &shown, 12.0);
            if save {
                this.s.ui_language = languages()[draft.language.min(languages().len() - 1)].code.to_owned();
                this.s.accessibility_text_scale = SIZES[draft.size.min(SIZES.len() - 1)];
                this.s.accessibility_high_contrast = draft.contrast;
                this.save();
                this.flush();
                this.restart_note = Some(
                    "Saved. Quit and reopen NabyScape to apply language, text size and contrast.".to_owned(),
                );
            }
            this.lang_draft = Some(draft);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::settings::Settings;

    #[test]
    fn draft_reflects_the_saved_preferences() {
        let mut s = Settings::default();
        s.ui_language = "id".into();
        s.accessibility_text_scale = 1.2;
        s.accessibility_high_contrast = true;
        let d = LanguageDraft::of(&s);
        assert_eq!(
            (d.language, d.size, d.contrast),
            (2, 2, true),
            "1.2 rounds up to 125%"
        );
        assert_eq!(LanguageDraft::of(&Settings::default()).size, 0);
    }
}
