//! The `system` page (`SystemMonitorView.cs`, `SystemMonitorOverlay.cs`): tabs for the overview
//! (CPU with a 30 point history, memory, GPU, uptime), hardware, network, storage and
//! processes. It asks the host to sample only while it is shown and tells it which tab is open.

use super::common::{Ctl, Interaction, Presence};
use super::{Page, PageCtx};
use crate::host::{Demand, SystemInfo};
use naby_core::color::Rgba;
use naby_core::command::Command;
use naby_core::system::{self, MonitorHardware};
use naby_render::{Canvas, Icon, InputState, Rect, Weight};

const TAB_BASE: u32 = 10;
const DETAILS: u32 = 30;

/// `#28FFFFFF`: white at 16%.
const WASH: Rgba = Rgba::WHITE.with_alpha(0x28);

pub fn build() -> Box<dyn Page> {
    Box::new(SystemPage::default())
}

/// The tabs in order, with the setting that gates the optional ones.
pub fn tabs(settings: &naby_core::settings::Settings) -> Vec<(&'static str, &'static str)> {
    [
        ("overview", "Overview", true),
        ("hardware", "Hardware", true),
        ("network", "Network", settings.monitor_network),
        ("storage", "Storage", settings.monitor_storage),
        ("processes", "Processes", settings.monitor_processes),
    ]
    .into_iter()
    .filter(|t| t.2)
    .map(|t| (t.0, t.1))
    .collect()
}

/// One block of the detail area, already formatted.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Row {
        label: String,
        value: String,
        prominent: bool,
    },
    Section(String),
    /// Muted note text in `size` DIP with `top` DIP above it.
    Note {
        text: String,
        size: f32,
        top: f32,
    },
    /// A 12 DIP heading on one line (model names), `bottom` DIP below.
    Model {
        text: String,
        bottom: f32,
    },
    History,
    Memory {
        used: f32,
    },
    Details(Vec<String>),
}

#[derive(Default)]
struct SystemPage {
    ui: Interaction,
    presence: Presence,
    tab: String,
    details_open: bool,
    items: Vec<Item>,
    key: Option<(u64, String, u8, bool, bool)>,
}

/// Rows of the page for a tab: the `RenderSystemMonitor` of the WPF build.
fn build_items(
    tab: &str,
    info: &SystemInfo,
    settings: &naby_core::settings::Settings,
    details_open: bool,
) -> Vec<Item> {
    let _ = details_open;
    let mut items = Vec::new();
    let row = |label: &str, value: String, prominent: bool| Item::Row {
        label: label.to_owned(),
        value,
        prominent,
    };
    let Some(s) = info.latest.as_deref() else {
        items.push(Item::Note {
            text: "Sampling system information\u{2026}".to_owned(),
            size: 11.0,
            top: 0.0,
        });
        return items;
    };
    if tab == "overview" {
        items.push(row("CPU", system::percent(info.cpu), true));
        items.push(Item::History);
        items.push(Item::Note {
            text: "CPU \u{b7} 60 second window \u{b7} 2 second samples".to_owned(),
            size: 10.0,
            top: 0.0,
        });
        match (s.total, s.available) {
            (Some(total), Some(available)) if available <= total => {
                items.push(row(
                    "Memory",
                    format!(
                        "{} / {}",
                        system::bytes((total - available) as f64),
                        system::bytes(total as f64)
                    ),
                    true,
                ));
                items.push(Item::Memory {
                    used: (total - available) as f32 / total.max(1) as f32,
                });
            }
            _ => items.push(row("Physical memory", "Unavailable".to_owned(), false)),
        }
        if let Some(h) = &s.hardware {
            let busiest = h
                .gpus
                .iter()
                .filter_map(|g| g.utilization)
                .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
            if busiest.is_some() {
                items.push(row("GPU", system::hardware_percent(busiest), true));
            }
        }
        items.push(row("System uptime", system::uptime(s.uptime), false));
    }
    if tab == "network" && settings.monitor_network {
        items.push(Item::Section("NETWORK \u{b7} RECEIVE / SEND".to_owned()));
        for net in &info.networks {
            items.push(row(
                &net.name,
                format!("{} \u{2193}", system::rate_text(net.received)),
                false,
            ));
            items.push(row(
                "",
                format!("{} \u{2191}", system::rate_text(net.sent)),
                false,
            ));
        }
        if info.networks.is_empty() {
            items.push(row("Active interfaces", "None available".to_owned(), false));
        }
    }
    if tab == "storage" && settings.monitor_storage {
        items.push(Item::Section(
            "FIXED VOLUMES \u{b7} FREE / TOTAL".to_owned(),
        ));
        for drive in &s.drives {
            items.push(row(
                &drive.name,
                format!(
                    "{} / {}",
                    system::bytes(drive.free as f64),
                    system::bytes(drive.total as f64)
                ),
                false,
            ));
        }
        if s.drives.is_empty() {
            items.push(row("Volumes", "None available".to_owned(), false));
        }
    }
    if tab == "processes" && settings.monitor_processes {
        items.push(row(
            "Processes / threads",
            system::process_counts(s.process_count, s.thread_count, s.partial_threads),
            false,
        ));
        items.push(Item::Section("TOP PROCESSES \u{b7} WORKING SET".to_owned()));
        for p in &s.processes {
            items.push(row(
                &format!("{} \u{b7} {}", p.name, p.id),
                system::bytes(p.working_set as f64),
                false,
            ));
        }
        if s.processes.is_empty() {
            items.push(row("Processes", "Unavailable".to_owned(), false));
        }
        items.push(Item::Note {
            text: "Working sets can share memory; they do not sum to total RAM usage.".to_owned(),
            size: 10.0,
            top: 6.0,
        });
    }
    if tab == "hardware" {
        hardware_items(&mut items, s.hardware.as_ref());
    }
    if !s.errors.is_empty() {
        items.push(Item::Note {
            text: s.errors.join(" \u{b7} "),
            size: 10.0,
            top: 8.0,
        });
    }
    items
}

/// `RenderHardware`.
fn hardware_items(items: &mut Vec<Item>, hardware: Option<&MonitorHardware>) {
    let row = |label: &str, value: String, prominent: bool| Item::Row {
        label: label.to_owned(),
        value,
        prominent,
    };
    let Some(h) = hardware else {
        items.push(row("Hardware", "Unavailable".to_owned(), false));
        return;
    };
    items.push(Item::Section("PROCESSOR".to_owned()));
    for cpu in &h.cpus {
        items.push(Item::Model {
            text: if cpu.name.is_empty() {
                "Processor unavailable".to_owned()
            } else {
                cpu.name.clone()
            },
            bottom: 0.0,
        });
        let count = |v: Option<i32>| v.map_or("\u{2014}".to_owned(), |n| n.to_string());
        let clock = match cpu.max_clock {
            Some(mhz) if mhz > 0.0 => format!(" \u{b7} {mhz:.0} MHz"),
            _ => String::new(),
        };
        items.push(row(
            "Cores / threads",
            format!("{} / {}{clock}", count(cpu.cores), count(cpu.threads)),
            false,
        ));
    }
    if h.cpus.is_empty() {
        items.push(row("CPU details", "Unavailable".to_owned(), false));
    }
    items.push(Item::Section("GRAPHICS".to_owned()));
    for model in &h.gpu_models {
        items.push(Item::Model {
            text: model.clone(),
            bottom: 4.0,
        });
    }
    if h.gpu_models.is_empty() {
        items.push(row("GPU model", "Unavailable".to_owned(), false));
    }
    for (i, gpu) in h.gpus.iter().enumerate() {
        items.push(row(
            &if h.gpus.len() > 1 {
                format!("GPU counter {}", i + 1)
            } else {
                "GPU utilization".to_owned()
            },
            system::hardware_percent(gpu.utilization),
            true,
        ));
        items.push(row(
            "Dedicated memory",
            gpu.dedicated
                .map_or("Unavailable".to_owned(), |d| system::bytes(d as f64)),
            false,
        ));
    }
    if h.gpus.is_empty() {
        items.push(row("GPU utilization", "Unavailable".to_owned(), false));
        items.push(row(
            "Dedicated / shared memory",
            "Unavailable".to_owned(),
            false,
        ));
    }
    if !h.sensors.is_empty() {
        items.push(Item::Section("TEMPERATURES & FANS".to_owned()));
        for sensor in &h.sensors {
            items.push(row(&sensor.name, system::sensor_value(sensor), false));
        }
    }
    let mut lines = Vec::new();
    for gpu in &h.gpus {
        lines.push(format!("Counter: {}", gpu.id));
        lines.push(format!(
            "Shared memory: {}",
            gpu.shared
                .map_or("Unavailable".to_owned(), |s| system::bytes(s as f64))
        ));
    }
    lines.push(
        "Utilization uses the busiest GPU engine. Counter IDs do not identify the model above."
            .to_owned(),
    );
    lines.push(if h.sensors.is_empty() {
        "Temperature and fan sensors need an already running LibreHardwareMonitor or OpenHardwareMonitor WMI provider."
            .to_owned()
    } else {
        let mut providers: Vec<&str> = Vec::new();
        for s in &h.sensors {
            if !providers.contains(&s.provider.as_str()) {
                providers.push(&s.provider);
            }
        }
        format!("Sensors: {}", providers.join(" / "))
    });
    if let Some(status) = &h.status {
        lines.push(status.clone());
    }
    items.push(Item::Details(lines));
}

impl SystemPage {
    fn refresh(&mut self, ctx: &PageCtx) {
        let s = ctx.settings();
        let available = tabs(s);
        if !available.iter().any(|t| t.0 == self.tab) {
            self.tab = "overview".to_owned();
        }
        let flags = (s.monitor_network as u8)
            | (s.monitor_storage as u8) << 1
            | (s.monitor_processes as u8) << 2;
        let key = (
            ctx.snaps.system.version(),
            self.tab.clone(),
            flags,
            self.details_open,
            true,
        );
        if self.key.as_ref() != Some(&key) {
            self.items = build_items(&self.tab, &ctx.snaps.system, s, self.details_open);
            self.key = Some(key);
        }
    }

    /// Lays out and (when drawing) paints; returns the height.
    fn layout(
        c: &mut Ctl,
        rect: Rect,
        tab: &mut String,
        details_open: &mut bool,
        items: &[Item],
        info: &SystemInfo,
    ) -> f32 {
        let th = c.theme().clone();
        let s = c.s;
        let mut y = rect.y;
        let sel_fill = th.selection_fill;
        let sel_text = th.selection_text;

        // ---- tabs (WrapPanel of 28 DIP buttons, 3 DIP margins) ----------------------------
        let available = tabs(c.ctx.settings());
        let mut x = rect.x;
        for (i, (id, label)) in available.iter().enumerate() {
            let w = c.text_w(label, 10.0, Weight::Regular) + 14.0 * s;
            let h = (c.line_h(10.0, Weight::Regular) + 10.0 * s).max(28.0 * s);
            if x > rect.x && x + w > rect.right() {
                x = rect.x;
                y += 31.0 * s;
            }
            let r = Rect::new(x, y, w, h);
            let selected = *tab == *id;
            let (fill, color) = if selected {
                (Some(sel_fill), sel_text)
            } else {
                (None, th.text_dim)
            };
            let b = c.text_button(
                TAB_BASE + i as u32,
                r,
                label,
                10.0,
                Weight::Regular,
                color,
                fill,
                true,
            );
            if b.clicked && !selected {
                *tab = (*id).to_owned();
                c.push(Command::SystemTab((*id).to_owned()));
            }
            x += w + 3.0 * s;
        }
        y += 31.0 * s + 10.0 * s;

        // ---- detail rows --------------------------------------------------------------------
        let top = y;
        let value_cap = (92.0 * s).max(rect.w * 0.61);
        for item in items {
            match item {
                Item::Row {
                    label,
                    value,
                    prominent,
                } => {
                    let vs = if *prominent { 15.0 } else { 11.0 };
                    let h = c
                        .line_h(vs, Weight::Regular)
                        .max(c.line_h(11.0, Weight::Regular))
                        + 6.0 * s;
                    let vw = c
                        .text_w(value, vs, Weight::Regular)
                        .min(value_cap.max(90.0 * s));
                    let vy = y + 3.0 * s + (h - 6.0 * s - c.line_h(vs, Weight::Regular)) / 2.0;
                    let ny = y + 3.0 * s + (h - 6.0 * s - c.line_h(11.0, Weight::Regular)) / 2.0;
                    c.text(
                        rect.x,
                        ny,
                        label,
                        11.0,
                        Weight::Regular,
                        th.text_dim,
                        Some((rect.w - vw - 8.0 * s).max(1.0)),
                    );
                    c.text_right(
                        rect.right(),
                        vy,
                        value,
                        vs,
                        Weight::Regular,
                        th.text,
                        Some(vw),
                    );
                    y += h;
                }
                Item::Section(text) => {
                    y += 15.0 * s;
                    c.text(
                        rect.x,
                        y,
                        text,
                        11.0,
                        Weight::Regular,
                        th.text_dim,
                        Some(rect.w),
                    );
                    y += c.line_h(11.0, Weight::Regular) + 7.0 * s;
                }
                Item::Note { text, size, top } => {
                    y += top * s;
                    y += c.paragraph(rect.x, y, rect.w, text, *size, Weight::Regular, th.text_dim);
                }
                Item::Model { text, bottom } => {
                    let weight = if th.bold_headings {
                        Weight::Semibold
                    } else {
                        Weight::Regular
                    };
                    c.text(rect.x, y, text, 12.0, weight, th.text, Some(rect.w));
                    y += c.line_h(12.0, weight) + bottom * s;
                }
                Item::History => {
                    y += 4.0 * s;
                    let h = 42.0 * s;
                    if !c.is_dry() {
                        draw_history(
                            c,
                            Rect::new(rect.x, y, rect.w, h),
                            &info.cpu_history,
                            th.accent,
                        );
                    }
                    y += h + 2.0 * s;
                }
                Item::Memory { used } => {
                    y += 4.0 * s;
                    let r = Rect::new(rect.x, y, rect.w, 3.0 * s);
                    c.fill(r, WASH);
                    c.fill(
                        Rect::new(r.x, r.y, r.w * used.clamp(0.0, 1.0), r.h),
                        th.accent,
                    );
                    y += 3.0 * s + 8.0 * s;
                }
                Item::Details(lines) => {
                    y += 14.0 * s;
                    let label = "Sensor availability & technical details";
                    let h = c.line_h(11.0, Weight::Regular).max(20.0 * s);
                    let r = Rect::new(rect.x, y, rect.w, h);
                    let b = c.button(DETAILS, r, true);
                    c.button_chrome(r, 6.0, None, true, &b);
                    let cy = y + h / 2.0;
                    let icon = if *details_open {
                        Icon::Chevron
                    } else {
                        Icon::Next
                    };
                    c.icon_at(icon, rect.x + 8.0 * s, cy, 8.0, th.text_dim);
                    let ly = y + (h - c.line_h(11.0, Weight::Regular)) / 2.0;
                    c.text(
                        rect.x + 22.0 * s,
                        ly,
                        label,
                        11.0,
                        Weight::Regular,
                        th.text_dim,
                        Some(rect.w - 22.0 * s),
                    );
                    if b.clicked {
                        *details_open = !*details_open;
                    }
                    y += h;
                    if *details_open {
                        y += 6.0 * s;
                        for line in lines {
                            y += c.paragraph(
                                rect.x + 22.0 * s,
                                y,
                                rect.w - 22.0 * s,
                                line,
                                10.0,
                                Weight::Regular,
                                th.text_dim,
                            );
                        }
                    }
                }
            }
        }
        y = y.max(top + 92.0 * s);
        y - rect.y
    }
}

/// The 30 point CPU line: baseline plus polyline segments between consecutive readings.
fn draw_history(c: &mut Ctl, area: Rect, history: &[Option<f64>], accent: Rgba) {
    let s = c.s;
    c.fill(
        Rect::new(area.x, area.y + 41.0 * s, area.w, 1.0_f32.max(s)),
        WASH,
    );
    let count = history.len().min(30);
    let skip = history.len() - count;
    let mut run: Vec<(f32, f32)> = Vec::new();
    for (i, value) in history[skip..].iter().enumerate() {
        match value {
            Some(v) => {
                let x = area.x + area.w * (30 - count + i) as f32 / 29.0;
                let y = area.y + (40.0 - (*v as f32).clamp(0.0, 100.0) * 0.38) * s;
                run.push((x, y));
            }
            None => {
                c.stroke_polyline(&run, accent, 1.6 * s);
                run.clear();
            }
        }
    }
    c.stroke_polyline(&run, accent, 1.6 * s);
}

impl Page for SystemPage {
    fn id(&self) -> &'static str {
        "system"
    }

    fn height(&mut self, ctx: &PageCtx, width: f32) -> f32 {
        self.refresh(ctx);
        let input = InputState::default();
        let mut c = Ctl::measure(ctx, &input, &mut self.ui);
        Self::layout(
            &mut c,
            Rect::new(0.0, 0.0, width, 0.0),
            &mut self.tab,
            &mut self.details_open,
            &self.items,
            &ctx.snaps.system,
        )
    }

    fn frame(
        &mut self,
        canvas: &mut Canvas,
        ctx: &PageCtx,
        rect: Rect,
        input: &InputState,
        out: &mut Vec<Command>,
    ) -> bool {
        self.presence.mark(ctx.snaps.now);
        self.refresh(ctx);
        let mut c = Ctl::draw(canvas, ctx, input, out, &mut self.ui);
        Self::layout(
            &mut c,
            rect,
            &mut self.tab,
            &mut self.details_open,
            &self.items,
            &ctx.snaps.system,
        );
        false
    }

    fn demand(&self) -> Demand {
        Demand {
            system: self.presence.active(),
            ..Demand::default()
        }
    }

    fn next_frame(&self, now: f64) -> Option<f64> {
        self.presence.observe(now);
        None
    }

    fn captures_pointer(&self) -> bool {
        self.ui.captures()
    }

    fn on_hide(&mut self) {
        self.presence.hide();
        self.ui.reset();
        self.items = Vec::new();
        self.key = None;
        self.tab = "overview".to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::NetRates;
    use crate::pages::common::harness::Harness;
    use naby_core::system::{
        HardwareCpu, HardwareGpu, HardwareSensor, MonitorDrive, MonitorNetwork, MonitorProcess,
        MonitorSnapshot,
    };
    use std::sync::Arc;
    use std::time::Duration;

    fn snapshot() -> MonitorSnapshot {
        MonitorSnapshot {
            timestamp: 1,
            idle: Some(10),
            cpu_total: Some(100),
            available: Some(6 << 30),
            total: Some(16 << 30),
            uptime: Duration::from_secs(3 * 86_400 + 4 * 3600 + 5 * 60),
            process_count: Some(212),
            thread_count: Some(3120),
            partial_threads: false,
            networks: vec![MonitorNetwork {
                id: "a".into(),
                name: "Ethernet".into(),
                received: 1,
                sent: 2,
            }],
            drives: vec![MonitorDrive {
                name: "C:\\".into(),
                free: 120 << 30,
                total: 500 << 30,
            }],
            processes: vec![MonitorProcess {
                name: "chrome".into(),
                id: 4242,
                working_set: 900 << 20,
            }],
            errors: vec![],
            hardware: Some(MonitorHardware {
                cpus: vec![HardwareCpu {
                    name: "AMD Ryzen 7 5800X 8-Core Processor".into(),
                    cores: Some(8),
                    threads: Some(16),
                    max_clock: Some(3800.0),
                }],
                gpu_models: vec!["NVIDIA GeForce RTX 3070".into()],
                gpus: vec![HardwareGpu {
                    id: "luid_0x0_0x1".into(),
                    utilization: Some(37.5),
                    dedicated: Some(8 << 30),
                    shared: Some(16 << 30),
                }],
                sensors: vec![HardwareSensor {
                    name: "CPU Package".into(),
                    kind: "Temperature".into(),
                    value: 54.0,
                    provider: "LibreHardwareMonitor".into(),
                }],
                status: None,
            }),
        }
    }

    fn feed(h: &mut Harness) {
        let history: Vec<Option<f64>> = (0..30)
            .map(|i| {
                if i == 12 {
                    None
                } else {
                    Some(20.0 + (i as f64 * 0.7).sin() * 15.0 + i as f64)
                }
            })
            .collect();
        h.snaps.system.replace(SystemInfo {
            latest: Some(Arc::new(snapshot())),
            cpu: Some(41.25),
            cpu_history: history,
            networks: vec![NetRates {
                id: "a".into(),
                name: "Ethernet".into(),
                received: Some(1_500_000.0),
                sent: None,
            }],
            reads: 3,
        });
    }

    fn all_tabs(h: &mut Harness) {
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.monitor_network = true;
        s.monitor_storage = true;
        s.monitor_processes = true;
        h.snaps.settings.replace(s);
    }

    #[test]
    fn asks_the_host_for_samples_only_while_shown() {
        let Some(mut h) = Harness::new("system") else {
            return;
        };
        assert!(!h.page.demand().system, "not drawn yet");
        h.idle();
        assert!(h.page.demand().system);
        h.page.next_frame(h.snaps.now + 20.0);
        assert!(!h.page.demand().system, "an unseen page stops sampling");
        h.idle();
        assert!(h.page.demand().system);
        h.page.on_hide();
        assert!(!h.page.demand().system);
    }

    #[test]
    fn tab_clicks_tell_the_host_which_tab_is_open() {
        let Some(mut h) = Harness::new("system") else {
            return;
        };
        all_tabs(&mut h);
        feed(&mut h);
        h.idle();
        for (i, id) in ["overview", "hardware", "network", "storage", "processes"]
            .iter()
            .enumerate()
        {
            h.click_id(TAB_BASE + i as u32);
            let cmds = h.commands();
            if *id == "overview" {
                assert!(cmds.is_empty(), "already selected");
            } else {
                assert_eq!(cmds, vec![Command::SystemTab((*id).to_owned())]);
            }
        }
    }

    #[test]
    fn disabled_tabs_are_not_offered() {
        let Some(mut h) = Harness::new("system") else {
            return;
        };
        feed(&mut h);
        let mut s = h.snaps.settings.arc().as_ref().clone();
        s.monitor_network = false;
        s.monitor_storage = true;
        s.monitor_processes = false;
        h.snaps.settings.replace(s);
        h.idle();
        let ids: Vec<_> = tabs(&h.snaps.settings).iter().map(|t| t.0).collect();
        assert_eq!(ids, ["overview", "hardware", "storage"]);
        assert!(h.has_control(TAB_BASE + 2));
        assert!(!h.has_control(TAB_BASE + 3), "only three tabs are offered");
    }

    #[test]
    fn rows_read_like_the_wpf_build() {
        let s = naby_core::settings::Settings {
            monitor_network: true,
            monitor_storage: true,
            monitor_processes: true,
            ..Default::default()
        };
        let info = SystemInfo {
            latest: Some(Arc::new(snapshot())),
            cpu: Some(41.25),
            cpu_history: vec![Some(10.0)],
            networks: vec![NetRates {
                id: "a".into(),
                name: "Ethernet".into(),
                received: Some(1_500_000.0),
                sent: None,
            }],
            reads: 1,
        };
        let rows = |tab: &str| -> Vec<(String, String)> {
            build_items(tab, &info, &s, false)
                .into_iter()
                .filter_map(|i| match i {
                    Item::Row { label, value, .. } => Some((label, value)),
                    _ => None,
                })
                .collect()
        };
        let overview = rows("overview");
        assert_eq!(overview[0], ("CPU".into(), "41.3%".into()));
        assert_eq!(overview[1], ("Memory".into(), "10 GiB / 16 GiB".into()));
        assert_eq!(overview[2], ("GPU".into(), "37.5%".into()));
        assert_eq!(overview[3], ("System uptime".into(), "3d 4h 5m".into()));
        let network = rows("network");
        assert_eq!(network[0], ("Ethernet".into(), "1.4 MiB/s \u{2193}".into()));
        assert_eq!(network[1], ("".into(), "Sampling\u{2026} \u{2191}".into()));
        assert_eq!(
            rows("storage")[0],
            ("C:\\".into(), "120 GiB / 500 GiB".into())
        );
        let processes = rows("processes");
        assert_eq!(
            processes[0],
            ("Processes / threads".into(), "212 / 3120".into())
        );
        assert_eq!(
            processes[1],
            ("chrome \u{b7} 4242".into(), "900 MiB".into())
        );
        let hardware = rows("hardware");
        assert!(hardware.contains(&("Cores / threads".into(), "8 / 16 \u{b7} 3800 MHz".into())));
        assert!(hardware.contains(&("CPU Package".into(), "54 \u{b0}C".into())));
    }

    #[test]
    fn no_data_says_sampling_and_missing_pieces_say_unavailable() {
        let Some(mut h) = Harness::new("system") else {
            return;
        };
        h.idle();
        assert!(h.ink() > 300);
        h.save("page-system-sampling");
        // A sample without memory, drives, processes or hardware paints "Unavailable" rows.
        let mut s = snapshot();
        s.total = None;
        s.available = None;
        s.hardware = None;
        s.errors = vec!["Process enumeration unavailable".into()];
        h.snaps.system.replace(SystemInfo {
            latest: Some(Arc::new(s)),
            ..SystemInfo::default()
        });
        h.idle();
        let items = build_items("overview", &h.snaps.system, &h.snaps.settings, false);
        assert!(items.iter().any(|i| matches!(i, Item::Row { label, value, .. } if label == "Physical memory" && value == "Unavailable")));
        h.save("page-system-unavailable");
    }

    #[test]
    fn empty_adapter_drive_and_process_lists_say_none_available() {
        let s = naby_core::settings::Settings {
            monitor_network: true,
            monitor_storage: true,
            monitor_processes: true,
            ..Default::default()
        };
        let mut sample = snapshot();
        sample.networks.clear();
        sample.drives.clear();
        sample.processes.clear();
        sample.process_count = None;
        let info = SystemInfo {
            latest: Some(Arc::new(sample)),
            ..SystemInfo::default()
        };
        let row = |tab: &str, label: &str| {
            build_items(tab, &info, &s, false)
                .into_iter()
                .find_map(|i| match i {
                    Item::Row {
                        label: l, value, ..
                    } if l == label => Some(value),
                    _ => None,
                })
        };
        assert_eq!(
            row("network", "Active interfaces").as_deref(),
            Some("None available")
        );
        assert_eq!(row("storage", "Volumes").as_deref(), Some("None available"));
        assert_eq!(
            row("processes", "Processes").as_deref(),
            Some("Unavailable")
        );
        assert_eq!(
            row("processes", "Processes / threads").as_deref(),
            Some("Unavailable")
        );
        // The CPU reads "Sampling..." until there is a baseline.
        assert_eq!(row("overview", "CPU").as_deref(), Some("Sampling\u{2026}"));
    }

    #[test]
    fn snapshots_of_every_tab() {
        let Some(mut h) = Harness::new("system") else {
            return;
        };
        all_tabs(&mut h);
        feed(&mut h);
        h.idle();
        for (i, id) in ["overview", "hardware", "network", "storage", "processes"]
            .iter()
            .enumerate()
        {
            h.click_id(TAB_BASE + i as u32);
            h.idle();
            assert!(h.ink() > 1500, "{id} draws");
            h.save(&format!("page-system-{id}"));
        }
        h.click_id(TAB_BASE + 1);
        h.idle();
        h.click_id(DETAILS);
        h.idle();
        h.save("page-system-hardware-details");
    }
}
