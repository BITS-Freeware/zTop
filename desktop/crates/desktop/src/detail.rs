// SPDX-License-Identifier: MIT OR Apache-2.0
//! A host's detail page: history charts for CPU, memory, load, disks,
//! network, temperatures and fans, and a bar per core. It opens with
//! the agent's own history and then follows the fleet poll.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::cairo;
use ztop_desktop_core::history::{self, Line, WINDOW_SECONDS};
use ztop_desktop_core::summary::uptime_text;
use ztop_desktop_core::{History, HostRecord};

use crate::processes::ProcessTable;

/// Line colours, readable on light and dark backgrounds (the GNOME
/// palette's 3 shades).
const PALETTE: [(f64, f64, f64); 6] = [
    (0.208, 0.518, 0.894), // blue
    (0.902, 0.380, 0.000), // orange
    (0.180, 0.761, 0.494), // green
    (0.569, 0.255, 0.675), // purple
    (0.961, 0.761, 0.067), // yellow
    (0.878, 0.106, 0.141), // red
];

/// Temperature sensors drawn, hottest first.
const TEMPERATURE_LINES: usize = 6;

/// How a chart's values read.
#[derive(Clone, Copy)]
enum Scale {
    Percent,
    Plain,
    Rate,
    Celsius,
    Rpm,
}

impl Scale {
    fn fixed_top(self) -> Option<f64> {
        matches!(self, Self::Percent).then_some(100.0)
    }

    fn text(self, value: f64) -> String {
        match self {
            Self::Percent => format!("{value:.0}%"),
            Self::Plain => format!("{value:.2}"),
            Self::Rate => history::rate_text(value),
            Self::Celsius => format!("{value:.0} °C"),
            Self::Rpm => format!("{value:.0} RPM"),
        }
    }
}

/// One chart: a drawing area over the lines it was last given, with a
/// legend that names each line and its newest value.
struct Chart {
    group: adw::PreferencesGroup,
    area: gtk::DrawingArea,
    legend: gtk::Label,
    top: gtk::Label,
    start: gtk::Label,
    lines: Rc<RefCell<Vec<Line>>>,
    /// Seconds across the chart: the held history, a minute at least,
    /// so a young agent's charts are not a sliver at the right edge.
    span: Rc<Cell<f64>>,
    scale: Scale,
}

impl Chart {
    fn new(title: &str, scale: Scale) -> Self {
        let lines: Rc<RefCell<Vec<Line>>> = Rc::default();
        let span = Rc::new(Cell::new(WINDOW_SECONDS as f64));
        let area = gtk::DrawingArea::builder()
            .content_height(140)
            .hexpand(true)
            .build();
        {
            let (lines, span) = (lines.clone(), span.clone());
            area.set_draw_func(move |area, cr, width, height| {
                draw(area, cr, width, height, &lines.borrow(), span.get(), scale);
            });
        }
        let top = gtk::Label::builder().xalign(0.0).build();
        top.add_css_class("caption");
        top.add_css_class("dim-label");
        top.add_css_class("numeric");
        let ages = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let start = gtk::Label::builder()
            .label(span_text(WINDOW_SECONDS as f64))
            .xalign(0.0)
            .hexpand(true)
            .build();
        let end = gtk::Label::builder().label("now").xalign(1.0).build();
        for label in [&start, &end] {
            label.add_css_class("caption");
            label.add_css_class("dim-label");
            ages.append(label);
        }
        let legend = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .use_markup(true)
            .build();
        legend.add_css_class("numeric");

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        body.append(&top);
        body.append(&area);
        body.append(&ages);
        body.append(&legend);
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("card");
        frame.append(&body);

        let group = adw::PreferencesGroup::builder().title(title).build();
        group.add(&frame);
        Self {
            group,
            area,
            legend,
            top,
            start,
            lines,
            span,
            scale,
        }
    }

    /// Shows `lines`, or hides the whole section when the host reports
    /// none of them.
    fn show(&self, lines: Vec<Line>) {
        let has_points = lines.iter().any(|l| !l.points.is_empty());
        self.group.set_visible(has_points);
        if !has_points {
            return;
        }
        let refs: Vec<&history::Points> = lines.iter().map(|l| &l.points).collect();
        let top = history::scale_top(&refs, self.scale.fixed_top());
        self.top
            .set_label(&format!("scale 0 to {}", self.scale.text(top)));
        let oldest = lines
            .iter()
            .filter_map(|l| l.points.first().map(|p| p.0))
            .fold(0.0_f64, f64::max);
        let span = ((oldest / 60.0).ceil() * 60.0).clamp(60.0, WINDOW_SECONDS as f64);
        self.span.set(span);
        self.start.set_label(&span_text(span));
        let legend: Vec<String> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| {
                let (r, g, b) = PALETTE[i % PALETTE.len()];
                let now = line.points.last()?.1;
                Some(format!(
                    "<span foreground=\"#{:02x}{:02x}{:02x}\">●</span> {} {}",
                    (r * 255.0) as u8,
                    (g * 255.0) as u8,
                    (b * 255.0) as u8,
                    glib_escape(&line.label),
                    glib_escape(&self.scale.text(now)),
                ))
            })
            .collect();
        self.legend.set_markup(&legend.join("    "));
        *self.lines.borrow_mut() = lines;
        self.area.queue_draw();
    }
}

fn span_text(seconds: f64) -> String {
    format!("{:.0} min ago", seconds / 60.0)
}

fn glib_escape(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

/// Grid lines at quarters of the scale, then each line oldest to
/// newest, left to right across the window.
fn draw(
    area: &gtk::DrawingArea,
    cr: &cairo::Context,
    width: i32,
    height: i32,
    lines: &[Line],
    span: f64,
    scale: Scale,
) {
    let (w, h) = (f64::from(width), f64::from(height));
    let ink = area.color();
    cr.set_line_width(1.0);
    cr.set_source_rgba(
        f64::from(ink.red()),
        f64::from(ink.green()),
        f64::from(ink.blue()),
        0.12,
    );
    for quarter in 0..=4 {
        let y = (h - 1.0) * f64::from(quarter) / 4.0 + 0.5;
        cr.move_to(0.0, y);
        cr.line_to(w, y);
    }
    let _ = cr.stroke();

    let refs: Vec<&history::Points> = lines.iter().map(|l| &l.points).collect();
    let top = history::scale_top(&refs, scale.fixed_top());
    let window = span.max(1.0);
    cr.set_line_width(1.6);
    cr.set_line_join(cairo::LineJoin::Round);
    for (i, line) in lines.iter().enumerate() {
        let (r, g, b) = PALETTE[i % PALETTE.len()];
        cr.set_source_rgb(r, g, b);
        let mut first = true;
        for &(age, value) in &line.points {
            let x = w * (1.0 - (age / window).clamp(0.0, 1.0));
            let y = h - (value / top).clamp(0.0, 1.0) * (h - 2.0) - 1.0;
            if first {
                cr.move_to(x, y);
                first = false;
            } else {
                cr.line_to(x, y);
            }
        }
        let _ = cr.stroke();
    }
}

/// One column per core, filled to its busy percent, on a faint track.
fn draw_cores(area: &gtk::DrawingArea, cr: &cairo::Context, width: i32, height: i32, busy: &[f64]) {
    if busy.is_empty() {
        return;
    }
    let (w, h) = (f64::from(width), f64::from(height));
    let count = busy.len() as f64;
    let gap = if count > 32.0 { 1.0 } else { 3.0 };
    let bar = ((w - gap * (count - 1.0)) / count).max(1.0);
    let ink = area.color();
    let (r, g, b) = PALETTE[0];
    for (i, value) in busy.iter().enumerate() {
        let x = i as f64 * (bar + gap);
        cr.set_source_rgba(
            f64::from(ink.red()),
            f64::from(ink.green()),
            f64::from(ink.blue()),
            0.08,
        );
        cr.rectangle(x, 0.0, bar, h);
        let _ = cr.fill();
        let filled = h * (value / 100.0).clamp(0.0, 1.0);
        cr.set_source_rgb(r, g, b);
        cr.rectangle(x, h - filled, bar, filled);
        let _ = cr.fill();
    }
}

/// The page and what it shows for one host.
pub struct Detail {
    pub id: String,
    pub page: adw::NavigationPage,
    scroller: gtk::ScrolledWindow,
    title: adw::WindowTitle,
    status: gtk::Label,
    cores: gtk::DrawingArea,
    core_values: Rc<RefCell<Vec<f64>>>,
    cpu: Chart,
    memory: Chart,
    memory_rows: gtk::Label,
    load: Chart,
    disks: Chart,
    disk_rows: gtk::ListBox,
    network: Chart,
    temperatures: Chart,
    fans: Chart,
    history: RefCell<History>,
    processes: Rc<ProcessTable>,
}

impl Detail {
    pub fn new(record: &HostRecord, token: Option<String>, toasts: &adw::ToastOverlay) -> Rc<Self> {
        let title = adw::WindowTitle::new(&record.name, &record.target());
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let status = gtk::Label::builder()
            .label("Reading the agent's history…")
            .xalign(0.0)
            .wrap(true)
            .build();
        status.add_css_class("dim-label");

        let cpu = Chart::new("CPU", Scale::Percent);
        let core_values: Rc<RefCell<Vec<f64>>> = Rc::default();
        let cores = gtk::DrawingArea::builder()
            .content_height(56)
            .hexpand(true)
            .visible(false)
            .build();
        cores.set_tooltip_text(Some("Busy percent per core"));
        {
            let values = core_values.clone();
            cores.set_draw_func(move |area, cr, width, height| {
                draw_cores(area, cr, width, height, &values.borrow());
            });
        }
        cpu.group.add(&cores);

        let memory = Chart::new("Memory", Scale::Percent);
        let memory_rows = gtk::Label::builder().xalign(0.0).wrap(true).build();
        memory_rows.add_css_class("caption");
        memory_rows.add_css_class("numeric");
        memory.group.add(&memory_rows);

        let load = Chart::new("Load average", Scale::Plain);
        let disks = Chart::new("Disks", Scale::Rate);
        let disk_rows = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        disk_rows.add_css_class("boxed-list");
        // Rows are read-only; a focusable list would pull the page's
        // scroll position down to it when the page opens.
        disk_rows.set_can_focus(false);
        disks.group.add(&disk_rows);
        let network = Chart::new("Network", Scale::Rate);
        let temperatures = Chart::new("Temperatures", Scale::Celsius);
        let fans = Chart::new("Fans", Scale::Rpm);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(18)
            .margin_bottom(24)
            .margin_start(18)
            .margin_end(18)
            .build();
        column.append(&status);
        let processes = ProcessTable::new(record, token, toasts);
        for chart in [&cpu, &memory, &load, &disks, &network, &temperatures, &fans] {
            chart.group.set_visible(false);
            column.append(&chart.group);
        }
        column.append(&processes.group);
        let clamp = adw::Clamp::builder()
            .maximum_size(960)
            .child(&column)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .vexpand(true)
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&scroller));
        let page = adw::NavigationPage::builder()
            .title(&record.name)
            .tag(format!("host-{}", record.id))
            .child(&view)
            .build();

        Rc::new(Self {
            id: record.id.clone(),
            page,
            scroller: scroller.clone(),
            title,
            status,
            cores,
            core_values,
            cpu,
            memory,
            memory_rows,
            load,
            disks,
            disk_rows,
            network,
            temperatures,
            fans,
            history: RefCell::default(),
            processes,
        })
    }

    /// The agent's history arrived (or failed, which leaves the page
    /// to fill from the poll).
    pub fn seed(&self, result: Result<Vec<ztop_core::model::Sample>, String>) {
        match result {
            Ok(samples) => {
                self.history.borrow_mut().extend(samples);
                self.status.set_visible(false);
            }
            Err(why) => self.status.set_label(&format!(
                "The agent's history could not be read ({why}); the charts start now."
            )),
        }
        self.refresh();
    }

    /// One new sample from the fleet poll.
    pub fn push(&self, sample: ztop_core::model::Sample) {
        // The history drops process lists; the table takes this one first.
        self.processes.show(&sample.processes);
        self.history.borrow_mut().push(sample);
        self.refresh();
    }

    /// Scrolls the page so the process table is at the top, for a
    /// recorded run of it.
    pub fn scroll_to_processes(&self) {
        let Some(content) = self.scroller.child() else {
            return;
        };
        if let Some(point) = self
            .processes
            .group
            .compute_point(&content, &gtk::graphene::Point::new(0.0, 0.0))
        {
            self.scroller
                .vadjustment()
                .set_value(f64::from(point.y()) - 12.0);
        }
    }

    /// The number of samples held, for the recorded-run report.
    pub fn samples(&self) -> usize {
        self.history.borrow().len()
    }

    /// The sections with something to show, for the recorded-run report.
    pub fn visible_sections(&self) -> Vec<String> {
        let mut names: Vec<String> = [
            &self.cpu,
            &self.memory,
            &self.load,
            &self.disks,
            &self.network,
            &self.temperatures,
            &self.fans,
        ]
        .iter()
        .filter(|c| c.group.is_visible())
        .map(|c| c.group.title().to_string())
        .collect();
        if self.processes.group.is_visible() {
            names.push("Processes".into());
        }
        if self.cores.is_visible() {
            names.push("per-core bars".into());
        }
        names
    }

    fn refresh(&self) {
        let history = self.history.borrow();
        let Some(latest) = history.latest() else {
            return;
        };
        let mut subtitle = vec![latest.system.clone()];
        if latest.uptime.available {
            subtitle.push(format!("up {}", uptime_text(latest.uptime.value)));
        }
        subtitle.retain(|s| !s.is_empty());
        self.title.set_subtitle(&subtitle.join(" · "));

        self.cpu.show(vec![Line {
            label: format!("busy, {} cores", latest.cpu.busy_percent.len()),
            points: history.cpu(),
        }]);
        self.show_cores(&latest.cpu.busy_percent);

        self.memory.show(history.memory());
        let memory = &latest.memory;
        let mut parts = Vec::new();
        for (label, metric) in [
            ("used", &memory.used),
            ("cache", &memory.cache),
            ("buffers", &memory.buffers),
            ("free", &memory.free),
            ("total", &memory.total),
        ] {
            if metric.available {
                parts.push(format!("{label} {}", history::bytes_text(metric.value)));
            }
        }
        if memory.swap_total.available && memory.swap_total.value > 0.0 {
            parts.push(format!(
                "swap {} of {}",
                history::bytes_text(memory.swap_used.value),
                history::bytes_text(memory.swap_total.value)
            ));
        }
        self.memory_rows.set_label(&parts.join("  ·  "));

        self.load.show(history.load());
        self.disks.show(history.disks());
        self.show_disks(latest);
        self.network.show(history.network());
        self.temperatures
            .show(history.temperatures(TEMPERATURE_LINES));
        self.fans.show(history.fans());
    }

    /// A bar per core, drawn by [`draw_cores`].
    fn show_cores(&self, busy: &[f64]) {
        self.cores.set_visible(!busy.is_empty());
        *self.core_values.borrow_mut() = busy.to_vec();
        self.cores.queue_draw();
    }

    /// One row per mounted disk: mount, use and current rates.
    fn show_disks(&self, latest: &ztop_core::model::Sample) {
        while let Some(row) = self.disk_rows.row_at_index(0) {
            self.disk_rows.remove(&row);
        }
        let mut disks: Vec<_> = latest
            .disks
            .iter()
            .filter(|d| !d.mount.is_empty() && d.total_bytes > 0)
            .collect();
        disks.sort_by(|a, b| a.mount.cmp(&b.mount));
        self.disk_rows.set_visible(!disks.is_empty());
        for disk in disks {
            let mut rates = Vec::new();
            if let Some(rate) = disk.read_rate {
                rates.push(format!("read {}", history::rate_text(rate)));
            }
            if let Some(rate) = disk.write_rate {
                rates.push(format!("write {}", history::rate_text(rate)));
            }
            let row = adw::ActionRow::builder()
                .title(glib_escape(&disk.mount))
                .subtitle(glib_escape(&format!(
                    "{} · {} of {}{}",
                    disk.name,
                    history::bytes_text(disk.used_bytes as f64),
                    history::bytes_text(disk.total_bytes as f64),
                    if rates.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", rates.join(", "))
                    }
                )))
                .build();
            let bar = gtk::LevelBar::for_interval(0.0, 100.0);
            bar.set_value(disk.usage_pct.clamp(0.0, 100.0));
            bar.set_width_request(120);
            bar.set_valign(gtk::Align::Center);
            let percent = gtk::Label::new(Some(&format!("{:.0}%", disk.usage_pct)));
            percent.add_css_class("numeric");
            row.add_suffix(&bar);
            row.add_suffix(&percent);
            self.disk_rows.append(&row);
        }
    }
}
