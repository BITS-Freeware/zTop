// SPDX-License-Identifier: MIT OR Apache-2.0
//! One fleet card: name, address, state, CPU and memory meters, uptime,
//! and the actions that fit the state (pair, remove).

use adw::prelude::*;
use ztop_desktop_core::{CardSummary, HostLive, HostRecord, HostState};

pub struct Card {
    pub root: gtk::FlowBoxChild,
    state: gtk::Label,
    system: gtk::Label,
    cpu: gtk::LevelBar,
    cpu_text: gtk::Label,
    memory: gtk::LevelBar,
    memory_text: gtk::Label,
    uptime: gtk::Label,
    error: gtk::Label,
    pub pair: gtk::Button,
    pub remove: gtk::Button,
}

fn meter(label: &str) -> (gtk::Box, gtk::LevelBar, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::builder()
        .label(label)
        .width_chars(4)
        .xalign(0.0)
        .build();
    name.add_css_class("dim-label");
    let bar = gtk::LevelBar::for_interval(0.0, 100.0);
    bar.set_hexpand(true);
    bar.set_valign(gtk::Align::Center);
    // The default offsets colour a nearly full bar as "good"; for load
    // the reverse is true, so the meter keeps one neutral colour.
    for offset in ["low", "high", "full"] {
        bar.remove_offset_value(Some(offset));
    }
    let text = gtk::Label::builder()
        .label("–")
        .width_chars(5)
        .xalign(1.0)
        .build();
    text.add_css_class("numeric");
    row.append(&name);
    row.append(&bar);
    row.append(&text);
    (row, bar, text)
}

impl Card {
    pub fn new(record: &HostRecord) -> Self {
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(14)
            .margin_bottom(14)
            .margin_start(16)
            .margin_end(16)
            .build();

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        let name = gtk::Label::builder()
            .label(&record.name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("heading");
        let address = gtk::Label::builder()
            .label(match &record.via {
                Some(_) => format!("through the hub at {}", record.target()),
                None => format!(
                    "{} · {}{}",
                    record.target(),
                    if record.pin.is_empty() {
                        "plain, loopback"
                    } else {
                        "TLS"
                    },
                    if record.hub { " · hub" } else { "" }
                ),
            })
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        address.add_css_class("dim-label");
        address.add_css_class("caption");
        titles.append(&name);
        titles.append(&address);
        let state = gtk::Label::new(Some(HostState::Connecting.label()));
        state.add_css_class("state-pill");
        state.set_valign(gtk::Align::Start);
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("flat");
        remove.set_valign(gtk::Align::Start);
        remove.set_tooltip_text(Some("Remove this host"));
        header.append(&titles);
        header.append(&state);
        header.append(&remove);

        let system = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        system.add_css_class("caption");
        let (cpu_row, cpu, cpu_text) = meter("CPU");
        let (memory_row, memory, memory_text) = meter("MEM");
        let uptime = gtk::Label::builder().xalign(0.0).build();
        uptime.add_css_class("caption");
        uptime.add_css_class("dim-label");
        let error = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        error.add_css_class("caption");
        error.add_css_class("error");
        let pair = gtk::Button::with_label("Pair…");
        pair.add_css_class("suggested-action");
        pair.set_halign(gtk::Align::Start);
        pair.set_visible(false);

        body.append(&header);
        body.append(&system);
        body.append(&cpu_row);
        body.append(&memory_row);
        // A hub's card counts its machines instead of metering itself.
        cpu_row.set_visible(!record.hub);
        memory_row.set_visible(!record.hub);
        system.set_visible(!record.hub);
        body.append(&uptime);
        body.append(&error);
        body.append(&pair);

        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("card");
        frame.append(&body);
        let root = gtk::FlowBoxChild::new();
        root.set_child(Some(&frame));
        root.set_focusable(false);

        let card = Self {
            root,
            state,
            system,
            cpu,
            cpu_text,
            memory,
            memory_text,
            uptime,
            error,
            pair,
            remove,
        };
        card.show(&HostLive::default());
        card
    }

    /// A hub's state and what it watches, in one line.
    pub fn show_hub(&self, state: HostState, summary: &str, error: Option<&str>) {
        self.set_state(state);
        self.uptime.set_label(summary);
        self.error.set_label(error.unwrap_or_default());
        self.error
            .set_visible(state != HostState::Live && error.is_some());
        self.pair.set_visible(state == HostState::NeedsPairing);
    }

    fn set_state(&self, state: HostState) {
        self.state.set_label(state.label());
        for class in ["live", "stale", "unreachable", "pairing", "connecting"] {
            self.state.remove_css_class(class);
        }
        self.state.add_css_class(match state {
            HostState::Live => "live",
            HostState::Stale => "stale",
            HostState::Unreachable => "unreachable",
            HostState::NeedsPairing => "pairing",
            HostState::Connecting => "connecting",
        });
    }

    /// Shows a host's latest state; stale and unreachable keep the last
    /// values on screen, flagged by the pill's colour.
    pub fn show(&self, live: &HostLive) {
        self.set_state(live.state);

        let summary = live.sample.as_ref().map(CardSummary::of);
        let percent = |value: Option<f64>, bar: &gtk::LevelBar, text: &gtk::Label| {
            bar.set_value(value.unwrap_or(0.0));
            text.set_label(&value.map_or("–".to_string(), |v| format!("{v:.0}%")));
        };
        percent(
            summary.as_ref().and_then(|s| s.cpu_percent),
            &self.cpu,
            &self.cpu_text,
        );
        percent(
            summary.as_ref().and_then(|s| s.memory_percent),
            &self.memory,
            &self.memory_text,
        );
        let system = summary
            .as_ref()
            .map(|s| s.system.clone())
            .unwrap_or_default();
        self.system.set_label(&system);
        self.system.set_visible(!system.is_empty());
        self.uptime.set_label(
            &summary
                .as_ref()
                .and_then(|s| s.uptime.clone())
                .map_or(String::new(), |u| format!("up {u}")),
        );

        let failing = matches!(
            live.state,
            HostState::Stale | HostState::Unreachable | HostState::NeedsPairing
        );
        self.error
            .set_label(live.error.as_deref().unwrap_or_default());
        self.error.set_visible(failing && live.error.is_some());
        self.pair.set_visible(live.state == HostState::NeedsPairing);
    }
}
