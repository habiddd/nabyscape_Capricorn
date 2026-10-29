//! Layout helpers from `MinimalWidgets.cs`: responsive header, tile grid, hover reveal and the
//! viewport test that gates thumbnail decoding.

use super::{capped, Dir, Ui};
use crate::canvas::Rect;

/// Columns of the tile grid (`TileFlow`): as many `min_tile` wide tiles as fit, 1 or 2.
pub fn tile_columns(width: f32, min_tile: f32, gap: f32) -> usize {
    (((width + gap) / (min_tile + gap)).floor().max(1.0) as usize).clamp(1, 2)
}

impl<'a> Ui<'a> {
    /// Heading on the left and actions on the right; below 330 px the actions drop under the
    /// heading (`MinimalWidgets.Header`). 18 px of space follows. The actions' width comes from
    /// the previous frame, so the first frame requests a repaint.
    pub fn header(&mut self, heading: impl FnOnce(&mut Ui<'a>), actions: impl FnOnce(&mut Ui<'a>)) {
        let id = self.next_id();
        let avail = self.available_width();
        if avail < 330.0 {
            self.column(|ui| {
                ui.spacing(12.0);
                heading(ui);
                actions(ui);
            });
        } else {
            let prev = self.mem.sizes.get(&id).map_or(0.0, |s| s.0);
            let mut width = 0.0;
            self.row(|ui| {
                ui.spacing(0.0);
                ui.column_w((avail - prev).max(0.0), heading);
                let (_, w, _) = ui.nested_sized(Dir::Row, None, None, actions);
                width = w;
            });
            if (width - prev).abs() > 0.5 {
                *capped(&mut self.mem.sizes, id) = (width, 0.0);
                self.mem.repaint = true;
            }
        }
        self.gap(18.0);
    }

    /// Tile grid (`TileFlow`): 1-2 columns with `min_tile` wide tiles and `gap` between them.
    /// Tiles for which `full_width(i)` is true take a whole row.
    pub fn tile_flow(
        &mut self,
        min_tile: f32,
        gap: f32,
        count: usize,
        full_width: impl Fn(usize) -> bool,
        mut tile: impl FnMut(&mut Ui<'a>, usize),
    ) {
        let width = self.available_width();
        let cols = tile_columns(width, min_tile, gap);
        let tile_w = ((width - gap * (cols as f32 - 1.0)) / cols as f32).max(0.0);
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut current: Vec<usize> = Vec::new();
        for i in 0..count {
            if full_width(i) {
                if !current.is_empty() {
                    rows.push(std::mem::take(&mut current));
                }
                rows.push(vec![i]);
            } else {
                current.push(i);
                if current.len() == cols {
                    rows.push(std::mem::take(&mut current));
                }
            }
        }
        if !current.is_empty() {
            rows.push(current);
        }
        self.column(|ui| {
            ui.spacing(gap);
            for row in rows {
                ui.row(|ui| {
                    ui.spacing(gap);
                    for i in row {
                        let w = if full_width(i) { width } else { tile_w };
                        ui.push_id(i, |ui| ui.column_w(w, |ui| tile(ui, i)));
                    }
                });
            }
        });
    }

    /// Runs `f` dimmed to 12% until the pointer is over `owner` or `keep` is true
    /// (`MinimalWidgets.Reveal`: row actions that appear on hover or keyboard focus).
    pub fn reveal_on_hover<R>(
        &mut self,
        owner: Rect,
        keep: bool,
        f: impl FnOnce(&mut Ui<'a>) -> R,
    ) -> R {
        let hot = keep || self.pointer_in(owner);
        let old = self.canvas.alpha();
        self.canvas.set_alpha(old * if hot { 1.0 } else { 0.12 });
        let r = f(self);
        self.canvas.set_alpha(old);
        r
    }

    /// True when `rect` is inside the enclosing scroll area's viewport extended by half a
    /// viewport above and below, the window in which thumbnails stay decoded.
    pub fn near_viewport(&self, rect: Rect) -> bool {
        let vp = self
            .scroll_ctx
            .last()
            .map_or_else(|| self.canvas.bounds(), |c| c.viewport);
        let wide = Rect::new(vp.x, vp.y - vp.h * 0.5, vp.w, vp.h * 2.0);
        rect.h > 0.0 && wide.intersect(&rect).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_columns_matches_tileflow() {
        assert_eq!(tile_columns(100.0, 150.0, 16.0), 1);
        assert_eq!(tile_columns(316.0, 150.0, 16.0), 2);
        assert_eq!(tile_columns(315.0, 150.0, 16.0), 1);
        assert_eq!(tile_columns(2000.0, 150.0, 16.0), 2);
    }
}
