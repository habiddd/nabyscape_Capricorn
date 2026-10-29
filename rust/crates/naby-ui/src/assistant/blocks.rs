//! Chat Markdown laid out and painted natively (`AssistantMarkdown.Render`): headings, lists,
//! quotes, rules, fenced code, bold/italic/code spans and links. No HTML or script is ever
//! interpreted; links are only hit regions that the view turns into `Command::OpenUrl` after
//! `is_safe_link`.
//!
//! A message is laid out once per width into a flat list of draw operations ([`Laid`]), which
//! the transcript caches and paints without re-measuring text.

use naby_core::assistant::markdown::{is_safe_link, Block, Bullet, Span};
use naby_core::color::Rgba;
use naby_render::{Canvas, Fonts, Rect, TextStyle, Weight};

use super::style::{Metrics, Palette};

/// A code block shows at most this many design units of lines (`MaxHeight=240`).
const CODE_MAX_HEIGHT: f32 = 240.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        weight: Weight,
        color: Rgba,
        mono: bool,
    },
    Fill {
        rect: Rect,
        radius: f32,
        color: Rgba,
    },
    Stroke {
        rect: Rect,
        radius: f32,
        color: Rgba,
        width: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkHit {
    pub rect: Rect,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CopyHit {
    pub rect: Rect,
    pub text: String,
}

/// One laid-out message body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Laid {
    pub height: f32,
    pub ops: Vec<Op>,
    pub links: Vec<LinkHit>,
    pub copies: Vec<CopyHit>,
}

/// What layout and painting measure with.
pub struct Ctx<'a> {
    pub fonts: &'a Fonts,
    pub mono: &'a Fonts,
    pub m: Metrics,
    pub pal: &'a Palette,
}

impl<'a> Ctx<'a> {
    fn mono_fonts(&self) -> &'a Fonts {
        if self.mono.has_faces() {
            self.mono
        } else {
            self.fonts
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    Bold,
    Italic,
    Code,
    Link,
}

struct Token {
    text: String,
    kind: Kind,
    link: Option<usize>,
    space: bool,
}

fn tokens(spans: &[Span], urls: &mut Vec<String>) -> Vec<Token> {
    let mut out = Vec::new();
    let mut push = |text: &str, kind: Kind, link: Option<usize>| {
        let mut word = String::new();
        let mut word_space = false;
        for c in text.chars() {
            let space = c == ' ' || c == '\t';
            if !word.is_empty() && space != word_space {
                out.push(Token {
                    text: std::mem::take(&mut word),
                    kind,
                    link,
                    space: word_space,
                });
            }
            word_space = space;
            word.push(c);
        }
        if !word.is_empty() {
            out.push(Token {
                text: word,
                kind,
                link,
                space: word_space,
            });
        }
    };
    for span in spans {
        match span {
            Span::Text(t) => push(t, Kind::Plain, None),
            Span::Bold(t) => push(t, Kind::Bold, None),
            Span::Italic(t) => push(t, Kind::Italic, None),
            Span::Code(t) => push(t, Kind::Code, None),
            Span::Link { text, url } => {
                let index = is_safe_link(url).then(|| {
                    urls.push(url.clone());
                    urls.len() - 1
                });
                push(text, Kind::Link, index);
            }
        }
    }
    out
}

struct Flow<'a> {
    ctx: &'a Ctx<'a>,
    laid: &'a mut Laid,
    urls: &'a [String],
}

impl Flow<'_> {
    /// Lays `tokens` out from (`x0`, `y0`) within `width`; returns the height used.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        tokens: &[Token],
        x0: f32,
        y0: f32,
        width: f32,
        size: f32,
        weight: Weight,
        color: Rgba,
        line_h: f32,
    ) -> f32 {
        let ctx = self.ctx;
        let pal = ctx.pal;
        let mut x = 0.0f32;
        let mut line = 0usize;
        let style_of = |kind: Kind| -> (f32, Weight, Rgba, bool) {
            match kind {
                Kind::Plain => (size, weight, color, false),
                Kind::Bold => (size, Weight::Bold, color, false),
                Kind::Italic => (size, weight, pal.dim.mix_toward(color, 0.6), false),
                Kind::Code => (size * 0.92, Weight::Regular, color, true),
                Kind::Link => (size, weight, pal.link, false),
            }
        };
        let measure = |text: &str, kind: Kind| -> f32 {
            let (s, w, c, mono) = style_of(kind);
            let fonts = if mono { ctx.mono_fonts() } else { ctx.fonts };
            fonts.measure_width(
                text,
                &TextStyle {
                    size: s,
                    weight: w,
                    color: c,
                },
            )
        };
        let place = |this: &mut Self,
                     text: &str,
                     kind: Kind,
                     link: Option<usize>,
                     x: f32,
                     line: usize|
         -> f32 {
            let (s, w, c, mono) = style_of(kind);
            let width = measure(text, kind);
            let y = y0 + line as f32 * line_h;
            if kind == Kind::Code {
                this.laid.ops.push(Op::Fill {
                    rect: Rect::new(x0 + x, y + 1.0, width, line_h - 2.0),
                    radius: 3.0 * ctx.m.s,
                    color: pal.code_inline,
                });
            }
            this.laid.ops.push(Op::Text {
                x: x0 + x,
                y,
                text: text.to_owned(),
                size: s,
                weight: w,
                color: c,
                mono,
            });
            if let Some(index) = link {
                let underline =
                    Rect::new(x0 + x, y + line_h - 4.0 * ctx.m.s, width, ctx.m.s.max(1.0));
                this.laid.ops.push(Op::Fill {
                    rect: underline,
                    radius: 0.0,
                    color: pal.link,
                });
                this.laid.links.push(LinkHit {
                    rect: Rect::new(x0 + x, y, width, line_h),
                    url: this.urls[index].clone(),
                });
            }
            width
        };
        for token in tokens {
            let w = measure(&token.text, token.kind);
            if token.space {
                if x > 0.0 {
                    if x + w <= width {
                        place(self, &token.text, token.kind, token.link, x, line);
                    }
                    x += w;
                }
                continue;
            }
            if x > 0.0 && x + w > width {
                x = 0.0;
                line += 1;
            }
            if w <= width || width <= 0.0 {
                x += place(self, &token.text, token.kind, token.link, x, line);
                continue;
            }
            // A word wider than the line is broken by character.
            let mut chunk = String::new();
            for c in token.text.chars() {
                let mut probe = chunk.clone();
                probe.push(c);
                if !chunk.is_empty() && measure(&probe, token.kind) > width - x {
                    place(self, &chunk, token.kind, token.link, x, line);
                    chunk.clear();
                    x = 0.0;
                    line += 1;
                }
                chunk.push(c);
            }
            if !chunk.is_empty() {
                x += place(self, &chunk, token.kind, token.link, x, line);
            }
        }
        (line + 1) as f32 * line_h
    }
}

/// Mixes two colors (used to tone italic text).
trait Mix {
    fn mix_toward(self, other: Rgba, amount: f32) -> Rgba;
}

impl Mix for Rgba {
    fn mix_toward(self, other: Rgba, amount: f32) -> Rgba {
        let lerp =
            |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
        Rgba {
            r: lerp(self.r, other.r),
            g: lerp(self.g, other.g),
            b: lerp(self.b, other.b),
            a: 255,
        }
    }
}

/// Lays an assistant reply out at `width` device pixels.
pub fn layout_markdown(ctx: &Ctx, blocks: &[Block], width: f32) -> Laid {
    let m = ctx.m;
    let pal = ctx.pal;
    let mut laid = Laid::default();
    let mut y = 0.0f32;
    let mut urls: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            Block::Spacer => y += m.px(6.0),
            Block::Rule => {
                y += m.px(7.0);
                laid.ops.push(Op::Fill {
                    rect: Rect::new(0.0, y, width, m.s.max(1.0)),
                    radius: 0.0,
                    color: pal.rule,
                });
                y += m.px(8.0);
            }
            Block::Code(code) => y += code_block(ctx, &mut laid, code, width, y),
            Block::Line {
                heading,
                quote,
                bullet,
                spans,
            } => {
                let title = *heading > 0;
                let (size, line_h) = if title {
                    if *heading <= 2 {
                        (m.font(17.0), m.px(24.0))
                    } else {
                        (m.font(14.0), m.px(24.0))
                    }
                } else {
                    (m.font(13.0), m.px(20.0))
                };
                let weight = if title {
                    Weight::Semibold
                } else {
                    Weight::Regular
                };
                let color = if *quote { pal.dim } else { pal.text };
                let (top, bottom) = if title {
                    (m.px(10.0), m.px(5.0))
                } else if bullet.is_some() {
                    (m.px(3.0), m.px(3.0))
                } else if *quote {
                    (m.px(4.0), m.px(4.0))
                } else {
                    (m.px(2.0), m.px(2.0))
                };
                y += top;
                let mut x = 0.0;
                if let Some(bullet) = bullet {
                    let label = match bullet {
                        Bullet::Dot => "\u{2022}".to_owned(),
                        Bullet::Number(n) => format!("{n}."),
                    };
                    let indent = m.px(10.0);
                    let style = TextStyle {
                        size,
                        weight: Weight::Regular,
                        color,
                    };
                    laid.ops.push(Op::Text {
                        x: indent,
                        y,
                        text: label.clone(),
                        size,
                        weight: Weight::Regular,
                        color,
                        mono: false,
                    });
                    x = indent + ctx.fonts.measure_width(&label, &style).max(m.px(8.0)) + m.px(6.0);
                }
                if *quote {
                    x += m.px(12.0);
                }
                let tokens = tokens(spans, &mut urls);
                let height = {
                    let mut flow = Flow {
                        ctx,
                        laid: &mut laid,
                        urls: &urls,
                    };
                    flow.run(
                        &tokens,
                        x,
                        y,
                        (width - x).max(m.px(40.0)),
                        size,
                        weight,
                        color,
                        line_h,
                    )
                };
                if *quote {
                    laid.ops.push(Op::Fill {
                        rect: Rect::new(0.0, y, m.px(2.0), height),
                        radius: 0.0,
                        color: pal.quote,
                    });
                }
                y += height + bottom;
            }
        }
    }
    laid.height = y;
    laid
}

fn code_block(ctx: &Ctx, laid: &mut Laid, code: &str, width: f32, y: f32) -> f32 {
    let m = ctx.m;
    let pal = ctx.pal;
    let fonts = ctx.mono_fonts();
    let size = m.font(11.0);
    let style = TextStyle {
        size,
        weight: Weight::Regular,
        color: pal.text,
    };
    let line_h = (fonts.line_height(&style)).max(size * 1.3).ceil();
    let pad_x = m.px(10.0);
    let pad_y = m.px(8.0);
    let top = m.px(4.0);
    let inner_w = (width - 2.0 * pad_x).max(m.px(40.0));
    let mut lines: Vec<String> = Vec::new();
    for raw in code.lines() {
        if raw.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(fonts.wrap(raw, &style, inner_w));
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    let max_h = m.px(CODE_MAX_HEIGHT) - 2.0 * pad_y;
    let fit = ((max_h / line_h).floor() as usize).max(1);
    let hidden = lines.len().saturating_sub(fit);
    let shown = lines.len().min(fit);
    let footer = if hidden > 0 { m.font(10.0) * 1.6 } else { 0.0 };
    let box_h = shown as f32 * line_h + 2.0 * pad_y + footer;
    let rect = Rect::new(0.0, y + top, width, box_h);
    laid.ops.push(Op::Fill {
        rect,
        radius: m.px(8.0),
        color: pal.control_fill,
    });
    laid.ops.push(Op::Stroke {
        rect,
        radius: m.px(8.0),
        color: pal.control_border,
        width: m.s.max(1.0),
    });
    for (i, line) in lines.iter().take(shown).enumerate() {
        if line.is_empty() {
            continue;
        }
        laid.ops.push(Op::Text {
            x: pad_x,
            y: rect.y + pad_y + i as f32 * line_h,
            text: line.clone(),
            size,
            weight: Weight::Regular,
            color: pal.text,
            mono: true,
        });
    }
    if hidden > 0 {
        laid.ops.push(Op::Text {
            x: pad_x,
            y: rect.y + pad_y + shown as f32 * line_h + m.px(2.0),
            text: format!("\u{2026} {hidden} more lines \u{b7} use Copy for all of it"),
            size: m.font(10.0),
            weight: Weight::Regular,
            color: pal.dim,
            mono: false,
        });
    }
    // The "Copy" affordance sits in the top-right corner.
    let label = "Copy";
    let copy_style = TextStyle {
        size: m.font(10.0),
        weight: Weight::Regular,
        color: pal.dim,
    };
    let label_w = ctx.fonts.measure_width(label, &copy_style);
    let button = Rect::new(
        rect.right() - label_w - m.px(18.0),
        rect.y + m.px(4.0),
        label_w + m.px(12.0),
        m.px(20.0),
    );
    laid.ops.push(Op::Text {
        x: button.x + m.px(6.0),
        y: button.y + (button.h - ctx.fonts.line_height(&copy_style)) / 2.0,
        text: label.to_owned(),
        size: copy_style.size,
        weight: Weight::Regular,
        color: pal.dim,
        mono: false,
    });
    laid.copies.push(CopyHit {
        rect: button,
        text: code.to_owned(),
    });
    top + box_h + m.px(4.0)
}

/// Plain text (user messages) wrapped at `width`, newlines kept.
pub fn layout_plain(ctx: &Ctx, text: &str, width: f32) -> Laid {
    let m = ctx.m;
    let size = m.font(13.0);
    let style = TextStyle {
        size,
        weight: Weight::Regular,
        color: ctx.pal.text,
    };
    let line_h = m.px(20.0);
    let mut laid = Laid::default();
    let mut y = 0.0;
    for line in ctx.fonts.wrap(text, &style, width) {
        if !line.is_empty() {
            laid.ops.push(Op::Text {
                x: 0.0,
                y,
                text: line,
                size,
                weight: Weight::Regular,
                color: ctx.pal.text,
                mono: false,
            });
        }
        y += line_h;
    }
    laid.height = y.max(line_h);
    laid
}

/// Paints the operations of `laid` with its origin at (`ox`, `oy`); operations entirely outside
/// the `[top, bottom]` band are skipped.
pub fn paint(laid: &Laid, canvas: &mut Canvas, ctx: &Ctx, ox: f32, oy: f32, band: (f32, f32)) {
    for op in &laid.ops {
        match op {
            Op::Fill {
                rect,
                radius,
                color,
            } => {
                let r = rect.translate(ox, oy);
                if r.bottom() >= band.0 && r.y <= band.1 {
                    canvas.fill_rrect(r, *radius, *color);
                }
            }
            Op::Stroke {
                rect,
                radius,
                color,
                width,
            } => {
                let r = rect.translate(ox, oy);
                if r.bottom() >= band.0 && r.y <= band.1 {
                    canvas.stroke_round_rect(r, *radius, *color, *width);
                }
            }
            Op::Text {
                x,
                y,
                text,
                size,
                weight,
                color,
                mono,
            } => {
                let top = oy + y;
                if top + size * 1.6 < band.0 || top > band.1 {
                    continue;
                }
                let fonts = if *mono { ctx.mono_fonts() } else { ctx.fonts };
                canvas.text(
                    fonts,
                    ox + x,
                    top,
                    text,
                    &TextStyle {
                        size: *size,
                        weight: *weight,
                        color: *color,
                    },
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::assistant::markdown;
    use naby_render::snapshot::{count_differing, test_fonts};
    use naby_render::Theme;

    fn palette() -> Palette {
        Palette::from_theme(&Theme::default())
    }

    #[test]
    fn markdown_layout_covers_every_block_kind() {
        let Some(fonts) = test_fonts() else { return };
        let mono = crate::assistant::style::load_mono();
        let pal = palette();
        let ctx = Ctx {
            fonts: &fonts,
            mono: &mono,
            m: Metrics::new(1.0, 1.0),
            pal: &pal,
        };
        let source = "# Heading\n\nSome **bold**, *italic*, `code` and a [link](https://example.com/a).\n\n- first\n- second item that is long enough to wrap onto another line when the width is narrow\n1. one\n> quoted\n---\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n";
        let blocks = markdown::parse(source);
        let wide = layout_markdown(&ctx, &blocks, 420.0);
        let narrow = layout_markdown(&ctx, &blocks, 160.0);
        assert!(wide.height > 100.0, "{}", wide.height);
        assert!(narrow.height > wide.height, "narrow text wraps taller");
        assert_eq!(wide.links.len(), 1);
        assert_eq!(wide.links[0].url, "https://example.com/a");
        assert_eq!(wide.copies.len(), 1);
        assert!(wide.copies[0].text.contains("println!"));
        assert!(wide
            .ops
            .iter()
            .any(|op| matches!(op, Op::Text { mono: true, .. })));

        let mut canvas = Canvas::new(440, wide.height.ceil() as u32 + 20);
        canvas.clear(pal.window);
        paint(&wide, &mut canvas, &ctx, 10.0, 10.0, (0.0, 1.0e6));
        let area = Rect::new(0.0, 0.0, canvas.width() as f32, canvas.height() as f32);
        assert!(count_differing(&canvas, area, pal.window) > 2000);
    }

    #[test]
    fn unsafe_links_are_text_not_hit_regions() {
        let Some(fonts) = test_fonts() else { return };
        let pal = palette();
        let mono = Fonts::empty();
        let ctx = Ctx {
            fonts: &fonts,
            mono: &mono,
            m: Metrics::new(1.0, 1.0),
            pal: &pal,
        };
        let blocks = markdown::parse("[x](https://user:pw@example.com/) [y](https://ok.example/)");
        let laid = layout_markdown(&ctx, &blocks, 400.0);
        let urls: Vec<&str> = laid.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, vec!["https://ok.example/"]);
    }

    #[test]
    fn long_words_and_code_are_bounded_by_the_width() {
        let Some(fonts) = test_fonts() else { return };
        let pal = palette();
        let mono = crate::assistant::style::load_mono();
        let ctx = Ctx {
            fonts: &fonts,
            mono: &mono,
            m: Metrics::new(1.0, 1.0),
            pal: &pal,
        };
        let long = "x".repeat(300);
        let blocks = markdown::parse(&format!("{long}\n```\n{}\n```", "y".repeat(500)));
        let laid = layout_markdown(&ctx, &blocks, 200.0);
        for op in &laid.ops {
            if let Op::Text {
                x,
                text,
                size,
                mono: is_mono,
                weight,
                color,
                ..
            } = op
            {
                let f = if *is_mono && mono.has_faces() {
                    &mono
                } else {
                    &fonts
                };
                let w = f.measure_width(
                    text,
                    &TextStyle {
                        size: *size,
                        weight: *weight,
                        color: *color,
                    },
                );
                assert!(x + w <= 201.0, "{text:.20} overflows: {}", x + w);
            }
        }
        // A tall code block is capped (with the "more lines" note).
        let tall = "z\n".repeat(200);
        let laid = layout_markdown(&ctx, &markdown::parse(&format!("```\n{tall}\n```")), 300.0);
        assert!(laid.height < 330.0, "{}", laid.height);
        assert!(laid
            .ops
            .iter()
            .any(|op| matches!(op, Op::Text { text, .. } if text.contains("more lines"))));
    }

    #[test]
    fn plain_text_keeps_newlines() {
        let Some(fonts) = test_fonts() else { return };
        let pal = palette();
        let mono = Fonts::empty();
        let ctx = Ctx {
            fonts: &fonts,
            mono: &mono,
            m: Metrics::new(1.0, 1.0),
            pal: &pal,
        };
        let laid = layout_plain(&ctx, "one\ntwo\n\nthree", 300.0);
        assert_eq!(laid.height, 80.0);
    }
}
