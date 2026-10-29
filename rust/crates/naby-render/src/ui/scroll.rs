//! Scroll area with the fading overlay scrollbar of `ScrollChrome.cs` (appears at 65% when the
//! content scrolls, fades out after 750 ms idle).

use super::{capped, Dir, Layout, ScrollCtx, Ui};
use crate::canvas::Rect;
use std::hash::Hash;

const GUTTER: f32 = 14.0;
const BAR_W: f32 = 12.0;
const SHOW_OPACITY: f32 = 0.65;
const IDLE_SECS: f64 = 0.75;
const FADE_SECS: f64 = 0.28;

#[derive(Clone, Copy, Default)]
pub(crate) struct ScrollState {
    pub offset: f32,
    pub content_h: f32,
    pub last_active: Option<f64>,
    pub grab: f32,
}

impl<'a> Ui<'a> {
    /// Scrolls its content inside the rest of the container (to the bottom of the region).
    pub fn scroll_area<R>(&mut self, salt: impl Hash, f: impl FnOnce(&mut Ui<'a>) -> R) -> R {
        let (_, y, _) = self.layouts.last().expect("layout").origin();
        let h = self
            .available_height()
            .min((self.canvas.height() as f32 - y).max(0.0));
        self.scroll_area_h(salt, h, f)
    }

    /// Scroll area of a fixed height.
    pub fn scroll_area_h<R>(
        &mut self,
        salt: impl Hash,
        height: f32,
        f: impl FnOnce(&mut Ui<'a>) -> R,
    ) -> R {
        let sid = self.hashed_id(salt);
        let rect = self.alloc(0.0, height.max(0.0), true);
        let now = self.input.time;
        let mut st = self.mem.scrolls.get(&sid).copied().unwrap_or_default();
        let overflow = st.content_h > rect.h + 0.5;
        let mut max_off = (st.content_h - rect.h).max(0.0);

        // Wheel.
        let hovered = self.pointer_in(rect);
        if hovered && self.input.wheel.1 != 0.0 {
            st.offset -= self.input.wheel.1;
            st.last_active = Some(now);
        }

        // Scrollbar thumb drag.
        let gutter = Rect::new(rect.right() - GUTTER, rect.y, GUTTER, rect.h);
        let track = Rect::new(
            rect.right() - BAR_W,
            rect.y + 8.0,
            BAR_W,
            (rect.h - 16.0).max(1.0),
        );
        let thumb_h = if st.content_h > 0.0 {
            (track.h * rect.h / st.content_h).clamp(24.0_f32.min(track.h), track.h)
        } else {
            track.h
        };
        let travel = (track.h - thumb_h).max(1.0);
        let thumb_y = track.y
            + travel
                * if max_off > 0.0 {
                    st.offset.clamp(0.0, max_off) / max_off
                } else {
                    0.0
                };
        let bar_id = self.hashed_id((sid, "bar"));
        let mut dragging = false;
        if overflow {
            let it = self.interact(bar_id, gutter, false);
            if let Some((_, py)) = self.input.pointer {
                if it.pressed_now {
                    st.grab = if py >= thumb_y && py <= thumb_y + thumb_h {
                        py - thumb_y
                    } else {
                        thumb_h / 2.0
                    };
                }
                if it.active && (it.pressed || it.pressed_now) {
                    st.offset = (py - track.y - st.grab) / travel * max_off;
                    st.last_active = Some(now);
                    dragging = true;
                }
            }
        }
        st.offset = st.offset.clamp(0.0, max_off);

        // Content.
        self.canvas.push_clip_rect(rect);
        self.clips.push(rect);
        let mut l = Layout::new(
            Dir::Column,
            Rect::new(
                rect.x,
                rect.y - st.offset,
                (rect.w - GUTTER).max(0.0),
                1.0e6,
            ),
        );
        l.spacing = self.layouts.last().map_or(0.0, |p| p.spacing);
        self.layouts.push(l);
        self.scroll_ctx.push(ScrollCtx {
            viewport: rect,
            reveal: None,
        });
        let result = f(self);
        let done = self.layouts.pop().expect("scroll layout");
        let ctx = self.scroll_ctx.pop().expect("scroll ctx");
        self.clips.pop();
        self.canvas.pop_clip();

        let new_h = done.used_h;
        if (new_h - st.content_h).abs() > 0.5 {
            st.content_h = new_h;
            self.mem.repaint = true;
            max_off = (new_h - rect.h).max(0.0);
        }
        if let Some(rv) = ctx.reveal {
            let before = st.offset;
            if rv.y < rect.y {
                st.offset -= rect.y - rv.y + 6.0;
            } else if rv.bottom() > rect.bottom() {
                st.offset += rv.bottom() - rect.bottom() + 6.0;
            }
            st.offset = st.offset.clamp(0.0, max_off);
            if (st.offset - before).abs() > 0.5 {
                self.mem.repaint = true;
            }
        }
        if st.offset > max_off {
            st.offset = max_off;
            self.mem.repaint = true;
        }

        // Overlay scrollbar.
        if st.content_h > rect.h + 0.5 {
            let alpha = match st.last_active {
                _ if dragging => SHOW_OPACITY,
                Some(t) => {
                    let since = now - t;
                    if since < IDLE_SECS {
                        self.mem.want_repaint_after(IDLE_SECS - since);
                        SHOW_OPACITY
                    } else if since < IDLE_SECS + FADE_SECS {
                        self.mem.want_repaint_after(0.03);
                        SHOW_OPACITY * (1.0 - ((since - IDLE_SECS) / FADE_SECS) as f32)
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };
            if alpha > 0.0 {
                let thumb_h =
                    (track.h * rect.h / st.content_h).clamp(24.0_f32.min(track.h), track.h);
                let travel = (track.h - thumb_h).max(0.0);
                let ty = track.y
                    + if max_off > 0.0 {
                        travel * st.offset / max_off
                    } else {
                        0.0
                    };
                let thumb = Rect::new(track.x + 4.0, ty, 4.0, thumb_h);
                let color = self.theme.text_dim;
                self.canvas
                    .with_opacity(alpha, |c| c.fill_rrect(thumb, 2.0, color));
            }
        }
        *capped(&mut self.mem.scrolls, sid) = st;
        result
    }
}
