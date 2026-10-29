//! The drag-to-reorder list of widgets (C# `SettingsWidgetOrder.cs`): a grip, the name and
//! explanation, and an enable switch per row. Drag the grip or press Alt+Up / Alt+Down on it.

use super::help::setting_description;
use super::SettingsView;
use naby_core::color::Rgba;
use naby_core::settings::{widget_display_name, WidgetSetting};
use naby_render::{Fill, Key, Rect, Ui, Weight};

/// Which ordered list a [`SettingsView::widget_order`] edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrderScope {
    /// `Settings.Widgets`.
    Widgets,
    /// `Settings.HoverViews`.
    Hover,
}

/// A grip drag in progress.
#[derive(Debug, Default, Clone)]
pub(crate) struct DragState {
    pub scope: Option<OrderScope>,
    pub id: String,
    pub active: bool,
    /// Row index under the pointer and whether the drop goes after it.
    pub target: Option<(usize, bool)>,
    start: Option<(f32, f32)>,
}

/// Distance the pointer must travel before a press on the grip becomes a drag
/// (`SystemParameters.MinimumVerticalDragDistance`).
const DRAG_START: f32 = 4.0;

impl SettingsView {
    fn order_list(&mut self, scope: OrderScope) -> &mut Vec<WidgetSetting> {
        match scope {
            OrderScope::Widgets => &mut self.s.widgets,
            OrderScope::Hover => &mut self.s.hover_views,
        }
    }

    /// `MoveTo(item, index)`.
    fn move_to(&mut self, scope: OrderScope, id: &str, index: usize) {
        let list = self.order_list(scope);
        let Some(old) = list.iter().position(|w| w.id == id) else {
            return;
        };
        let item = list.remove(old);
        let at = index.min(list.len());
        list.insert(at, item);
        self.save();
    }

    /// Draws the list. Rows are re-read from the live collection every frame.
    pub(super) fn widget_order<'a>(&mut self, ui: &mut Ui<'a>, scope: OrderScope) {
        let th = ui.theme().clone();
        let items: Vec<(String, bool)> = self
            .order_list(scope)
            .iter()
            .map(|w| (w.id.clone(), w.enabled))
            .collect();
        let pointer = ui.input().pointer;
        let released = ui.input().released || !ui.input().down;
        let mut drop_target: Option<(usize, bool)> = None;
        let mut moves: Vec<(String, usize)> = Vec::new();
        let mut toggles: Vec<(String, bool)> = Vec::new();
        let dragging_here = self.drag.active && self.drag.scope == Some(scope);
        for (index, (id, enabled)) in items.iter().enumerate() {
            let name = widget_display_name(id).unwrap_or(id.as_str()).to_owned();
            let label = self.tr(&name).to_owned();
            let help = setting_description(&name);
            let help_text = self.tr(&help).to_owned();
            let name_style = ui.text_style(12.0, Weight::Regular, th.text);
            let help_style = ui.text_style(10.0, Weight::Regular, th.text_dim);
            let avail = ui.available_width();
            let text_x = 5.0 + 28.0 + 8.0;
            let text_w = (avail - text_x - 12.0 - 46.0).max(40.0);
            let lh = ui.line_height(&name_style);
            let hl = ui.line_height(&help_style);
            let help_lines = if help.is_empty() {
                Vec::new()
            } else {
                ui.fonts().wrap(&help_text, &help_style, text_w)
            };
            let content = lh
                + if help_lines.is_empty() {
                    0.0
                } else {
                    4.0 + help_lines.len() as f32 * hl
                };
            let h = (content + 20.0 + 4.0).max(44.0);
            let card = ui.allocate(None, h);
            let dragged_row = dragging_here && self.drag.id == *id;
            ui.push_id(id.as_str(), |ui| {
                let faded = if dragged_row { 0.45 } else { 1.0 };
                // Drop indication: the pointer over the upper half inserts before.
                if dragging_here {
                    if let Some((_, py)) = pointer {
                        if let Some((px, _)) = pointer {
                            if card.contains(px, py) {
                                drop_target = Some((index, py > card.y + card.h / 2.0));
                            }
                        }
                    }
                }
                ui.canvas().with_opacity(faded, |c| {
                    let inner = Rect::new(card.x, card.y + 2.0, card.w, card.h - 4.0);
                    c.fill_rrect(inner, 7.0, Rgba::TRANSPARENT);
                });
                if let Some((t, after)) = self.drag.target.filter(|_| dragging_here) {
                    if t == index {
                        let y = if after { card.bottom() - 2.0 } else { card.y };
                        ui.canvas().fill_rrect(
                            Rect::new(card.x, y, card.w, 2.0),
                            1.0,
                            Rgba::parse_hex("#8099B7").unwrap_or(th.accent),
                        );
                    }
                }
                // Grip.
                let grip = Rect::new(card.x + 5.0, card.y + (card.h - 30.0) / 2.0, 28.0, 30.0);
                let resp = ui.hit_area(grip, true);
                self.hand |= resp.hovered;
                let tip = self
                    .tr("Drag to reorder · Alt+↑ / Alt+↓ with keyboard")
                    .to_owned();
                ui.tooltip(&resp, &tip);
                self.record(&format!("grip:{id}"), grip);
                if resp.pressed && !self.drag.active {
                    if let Some(p) = pointer {
                        match self.drag.start {
                            None => self.drag.start = Some(p),
                            Some(s) => {
                                if (p.1 - s.1).abs() >= DRAG_START
                                    || (p.0 - s.0).abs() >= DRAG_START
                                {
                                    self.drag.active = true;
                                    self.drag.scope = Some(scope);
                                    self.drag.id = id.clone();
                                }
                            }
                        }
                    }
                }
                if ui.visible(card) {
                    let dots = if resp.hovered || dragged_row {
                        th.text
                    } else {
                        th.text_dim
                    };
                    let c = ui.canvas();
                    c.with_opacity(faded, |c| {
                        for row in 0..3 {
                            for col in 0..2 {
                                c.fill_circle(
                                    grip.x + 10.0 + col as f32 * 7.0,
                                    grip.y + 8.0 + row as f32 * 7.0,
                                    1.5,
                                    &Fill::Solid(dots),
                                );
                            }
                        }
                    });
                    ui.paint_focus_ring(&resp, 6.0);
                    let mut y = card.y + (card.h - content) / 2.0;
                    let x = card.x + text_x;
                    ui.draw_text(x, y, &label, &name_style, Some(text_w));
                    y += lh;
                    if !help_lines.is_empty() {
                        y += 4.0;
                        for line in &help_lines {
                            ui.draw_text(x, y, line, &help_style, None);
                            y += hl;
                        }
                    }
                }
                // Keyboard reorder.
                if resp.focused {
                    for k in ui.input().keys.clone() {
                        if k.alt && matches!(k.key, Key::Up | Key::Down) {
                            let to = if k.key == Key::Up {
                                index.saturating_sub(1)
                            } else {
                                index + 1
                            };
                            if to < items.len() && to != index {
                                moves.push((id.clone(), to));
                            }
                        }
                    }
                }
                // Enable switch.
                let sw = Rect::new(
                    card.right() - 46.0,
                    card.y + (card.h - 40.0) / 2.0,
                    46.0,
                    40.0,
                );
                let sresp = ui.hit_area(sw, true);
                self.hand |= sresp.hovered;
                self.record(&format!("enable:{id}"), sw);
                if sresp.clicked {
                    toggles.push((id.clone(), !*enabled));
                }
                if ui.visible(card) {
                    let track = Rect::new(sw.right() - 40.0, sw.y + 10.0, 34.0, 20.0);
                    let c = ui.canvas();
                    c.with_opacity(faded, |c| {
                        c.fill_rrect(
                            track,
                            10.0,
                            if *enabled {
                                th.checked_fill
                            } else {
                                th.track_fill
                            },
                        );
                        let kx = if *enabled {
                            track.right() - 17.0
                        } else {
                            track.x + 3.0
                        };
                        c.fill_circle(
                            kx + 7.0,
                            track.y + 10.0,
                            7.0,
                            &Fill::Solid(if *enabled {
                                th.checked_text
                            } else {
                                th.switch_knob
                            }),
                        );
                        if sresp.hovered {
                            c.fill_rrect(track, 10.0, th.hover_fill);
                        }
                    });
                    ui.paint_focus_ring(&sresp, 8.0);
                }
            });
        }
        // Drag bookkeeping after the rows were laid out.
        if self.drag.active && self.drag.scope == Some(scope) {
            self.drag.target = drop_target.or(self.drag.target);
            self.repaint = true;
            if released {
                let dragged = self.drag.id.clone();
                if let Some((row, after)) = self.drag.target {
                    let mut index = row + usize::from(after);
                    let from = items.iter().position(|(i, _)| *i == dragged);
                    if let Some(from) = from {
                        if from < index {
                            index -= 1;
                        }
                        moves.push((dragged, index));
                    }
                }
                self.drag = DragState::default();
            } else if let Some((_, py)) = pointer {
                // Autoscroll near the edges of the page.
                let vp = self.page_viewport;
                if vp.h > 0.0 {
                    let offset = ui.memory().scroll_offset("page");
                    if py < vp.y + 40.0 {
                        ui.memory().set_scroll_offset("page", offset - 10.0);
                    } else if py > vp.bottom() - 40.0 {
                        ui.memory().set_scroll_offset("page", offset + 10.0);
                    }
                }
            }
        } else if released {
            self.drag.start = None;
        }
        for (id, value) in toggles {
            if let Some(w) = self.order_list(scope).iter_mut().find(|w| w.id == id) {
                w.enabled = value;
            }
            self.save();
        }
        for (id, index) in moves {
            self.move_to(scope, &id, index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::settings::Settings;

    #[test]
    fn move_to_reorders_the_live_list_and_schedules_a_save() {
        let mut view = SettingsView::new(Settings::default(), None);
        let first = view.s.widgets[0].id.clone();
        let second = view.s.widgets[1].id.clone();
        view.move_to(OrderScope::Widgets, &first, 3);
        assert_eq!(view.s.widgets[3].id, first);
        assert_eq!(view.s.widgets[0].id, second);
        assert!(view.save_at.is_some());
        view.move_to(OrderScope::Widgets, "nope", 0);
        assert_eq!(view.s.widgets[3].id, first);
    }
}
