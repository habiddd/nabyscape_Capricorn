//! The `links` page (C# `RoadmapOverlay.BuildCleanLinks`): paste a link, preview it without
//! tracking parameters and copy the result.

use super::tools_common::{
    base_key, hash_str, heading_weight, key_of, mark, readonly_box, tr, Shell,
};
use super::{Page, PageCtx};
use naby_core::command::Command;
use naby_core::productivity::links::try_clean_link;
use naby_render::{Canvas, InputState, Rect, TextEditOpts, Ui, Weight};

/// `input.MaxLength` of the link box.
pub const MAX_LINK_INPUT: usize = 8192;

pub fn build() -> Box<dyn Page> {
    Box::new(LinksPage {
        shell: Shell::new(),
        input: String::new(),
        result: String::new(),
        status: String::new(),
        copied: false,
        result_scroll: 0.0,
        analysed: None,
    })
}

struct LinksPage {
    shell: Shell,
    input: String,
    /// The cleaned link ("" while the input is blank or invalid).
    result: String,
    status: String,
    /// "Copied cleaned link." was the last status.
    copied: bool,
    result_scroll: f32,
    /// The input `result` and `status` were computed for.
    analysed: Option<String>,
}

/// Result and status text for an input (`input.TextChanged` handler).
pub fn analyse(ctx: &PageCtx, input: &str) -> (String, String) {
    if input.trim().is_empty() {
        return (String::new(), String::new());
    }
    match try_clean_link(input) {
        Ok(cleaned) => {
            let status = if cleaned == input.trim() {
                tr(ctx, "Unchanged \u{b7} already clean or signature protected")
            } else {
                tr(
                    ctx,
                    "Tracking parameters removed \u{b7} check the result below",
                )
            };
            (cleaned, status.to_owned())
        }
        Err(_) => (
            String::new(),
            super::tools_common::fmt(
                ctx,
                "Enter a complete HTTP or HTTPS URL without credentials (up to {0} characters).",
                &[&MAX_LINK_INPUT],
            ),
        ),
    }
}

impl LinksPage {
    fn refresh(&mut self, ctx: &PageCtx) {
        if self.analysed.as_deref() != Some(self.input.as_str()) {
            let (result, status) = analyse(ctx, &self.input);
            self.result = result;
            self.status = status;
            self.copied = false;
            self.analysed = Some(self.input.clone());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn layout(
    ui: &mut Ui,
    ctx: &PageCtx,
    input: &mut String,
    result: &str,
    status: &str,
    scroll: &mut f32,
    out: &mut Vec<Command>,
    copy: &mut bool,
) {
    let th = ui.theme().clone();
    let heading = ui.text_style(18.0, heading_weight(ctx), th.text);
    ui.label_styled(tr(ctx, "Clean links"), &heading);
    ui.gap(ctx.px(8.0));
    let hint = ui.text_style(11.0, Weight::Regular, th.text_dim);
    ui.label_styled(
        tr(
            ctx,
            "Preview a link without common tracking. Copy it when you're ready.",
        ),
        &hint,
    );
    ui.gap(ctx.px(8.0));
    let hint_text = tr(ctx, "Paste an http or https URL").to_owned();
    let r = ui.text_edit_with(
        input,
        &TextEditOpts::single()
            .hint(&hint_text)
            .max_len(MAX_LINK_INPUT),
    );
    mark("link-input", r.rect);
    ui.gap(ctx.px(4.0));
    // Status line.
    ui.gap(ctx.px(8.0));
    if status.is_empty() {
        let lh = ctx.fonts.line_height(&hint);
        ui.allocate(None, lh);
    } else {
        ui.label_styled(status, &hint);
    }
    ui.gap(ctx.px(8.0));
    let wheel = ui.input().wheel;
    let rect = readonly_box(ui, ctx, result, ctx.px(44.0), scroll, wheel);
    mark("link-result", rect);
    ui.gap(ctx.px(4.0));
    let enabled = !result.is_empty();
    let b = ui.enabled(enabled, |ui| {
        ui.primary_button(tr(ctx, "Copy cleaned link"))
    });
    mark("link-copy", b.rect);
    if enabled && b.clicked {
        out.push(Command::CopyText(result.to_owned()));
        *copy = true;
    }
}

impl Page for LinksPage {
    fn id(&self) -> &'static str {
        "links"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        self.refresh(ctx);
        let key = key_of(&[
            base_key(ctx),
            hash_str(&self.status),
            self.result.is_empty() as u64,
        ]);
        let (mut input, result, status) =
            (self.input.clone(), self.result.clone(), self.status.clone());
        let mut scroll = 0.0;
        self.shell.measure(ctx, width, key, |ui| {
            layout(
                ui,
                ctx,
                &mut input,
                &result,
                &status,
                &mut scroll,
                &mut Vec::new(),
                &mut false,
            );
        })
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        self.refresh(ctx);
        let mut copied = false;
        let (text, result, status, scroll) = (
            &mut self.input,
            self.result.clone(),
            self.status.clone(),
            &mut self.result_scroll,
        );
        let mut commands = Vec::new();
        let again = self.shell.run(canvas, ctx, rect, input, out, |ui| {
            layout(
                ui,
                ctx,
                text,
                &result,
                &status,
                scroll,
                &mut commands,
                &mut copied,
            );
        });
        out.append(&mut commands);
        if copied {
            self.status = tr(ctx, "Copied cleaned link.").to_owned();
            self.copied = true;
        }
        let edited = self.analysed.as_deref() != Some(self.input.as_str());
        again || copied || edited
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.shell.next_frame().map(|t| t.max(now))
    }

    fn captures_pointer(&self) -> bool {
        self.shell.captures_pointer()
    }

    fn on_hide(&mut self) {
        self.shell.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::tools_common::test_support::Rig;

    const TRACKED: &str = "https://example.com/page?id=7&utm_source=news&fbclid=abc#top";

    #[test]
    fn a_tracked_link_is_cleaned_and_copied() {
        let Some(mut rig) = Rig::new("links") else {
            return;
        };
        rig.paint();
        rig.click("link-input");
        rig.type_text(TRACKED);
        rig.paint();
        rig.save("page-links-cleaned");
        rig.click("link-copy");
        assert_eq!(
            rig.commands(),
            vec![Command::CopyText(
                "https://example.com/page?id=7#top".into()
            )]
        );
        rig.paint();
        rig.save("page-links-copied");
    }

    #[test]
    fn analysis_texts_match_the_csharp_status_lines() {
        let Some(rig) = Rig::new("links") else {
            return;
        };
        let ctx = rig.ctx();
        assert_eq!(analyse(&ctx, "  "), (String::new(), String::new()));
        let (r, s) = analyse(&ctx, TRACKED);
        assert_eq!(r, "https://example.com/page?id=7#top");
        assert_eq!(
            s,
            "Tracking parameters removed \u{b7} check the result below"
        );
        let (r, s) = analyse(&ctx, "https://example.com/a");
        assert_eq!(r, "https://example.com/a");
        assert_eq!(s, "Unchanged \u{b7} already clean or signature protected");
        let (r, s) = analyse(&ctx, "ftp://nope");
        assert!(r.is_empty());
        assert_eq!(
            s,
            "Enter a complete HTTP or HTTPS URL without credentials (up to 8192 characters)."
        );
    }

    #[test]
    fn the_copy_button_does_nothing_without_a_valid_link() {
        let Some(mut rig) = Rig::new("links") else {
            return;
        };
        rig.paint();
        rig.click("link-copy");
        assert!(rig.commands().is_empty());
        rig.click("link-input");
        rig.type_text("not a link");
        rig.paint();
        rig.click("link-copy");
        assert!(rig.commands().is_empty());
        rig.save("page-links-invalid");
    }
}
