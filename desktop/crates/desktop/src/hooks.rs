// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hooks for recorded runs and CI, read from the environment once the
//! saved hosts are loaded. They never run unless set.
//!
//! - `ZTOP_DESKTOP_SEED`: hosts to pair and add when no saved host has
//!   that name, as `name|address|port|fingerprint|code` entries joined
//!   by `;` (fingerprint and code empty for a plain loopback agent). A
//!   sixth field `hub` adds a hub.
//! - `ZTOP_DESKTOP_OPEN=add`: open the add-host dialog at start;
//!   `ZTOP_DESKTOP_OPEN=alerts`: open the alerts page.
//! - `ZTOP_DESKTOP_ALERT=<name>|<kind>|<threshold>|<seconds>`: add that
//!   rule for every host when no rule is saved (`kind` as in
//!   alerts.json: cpu, memory, load, disk, unreachable). Fired and
//!   cleared alerts are reported as `ztop-desktop: alert fired <rule>
//!   on <host>` lines.
//! - `ZTOP_DESKTOP_SIZE=<width>x<height>`: the window's size, so a
//!   recorded page fits in one picture.
//! - `ZTOP_DESKTOP_DETAIL=<name>`: open that host's detail page once
//!   it has a live sample; the snapshot report then names the sections
//!   the page shows and how many samples it holds.
//! - `ZTOP_DESKTOP_SEARCH=<text>`: start the process table's search
//!   with this text.
//! - `ZTOP_DESKTOP_SCROLL=processes`: once the detail page is open,
//!   scroll it to the process table.
//! - `ZTOP_DESKTOP_SNAPSHOT=<file.png>`: after
//!   `ZTOP_DESKTOP_SNAPSHOT_AFTER` seconds (default 8), draw the window
//!   into that PNG and quit. The app's own render, so no screen capture
//!   permission or compositor is needed. Each host's state and where
//!   tokens live are printed to stderr as `ztop-desktop: host <name>
//!   <state>` lines for scripts to check.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use ztop_desktop_core::{NewHost, agent};

use crate::app::Fleet;

pub fn run(fleet: &Rc<Fleet>) {
    let alert = std::env::var("ZTOP_DESKTOP_ALERT").unwrap_or_default();
    if !alert.trim().is_empty() {
        seed_alert(fleet, alert.trim());
    }
    match std::env::var("ZTOP_DESKTOP_OPEN").as_deref() {
        Ok("add") => crate::dialogs::add_host(fleet),
        Ok("alerts") => fleet.open_alerts(),
        _ => {}
    }
    if let Ok(seed) = std::env::var("ZTOP_DESKTOP_SEED") {
        seed_hosts(fleet, &seed);
    }
    if let Some((width, height)) = std::env::var("ZTOP_DESKTOP_SIZE").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }) {
        fleet.window.set_default_size(width, height);
    }
    if let Ok(name) = std::env::var("ZTOP_DESKTOP_DETAIL") {
        open_detail_when_live(fleet, name);
    }
    if let Ok(path) = std::env::var("ZTOP_DESKTOP_SNAPSHOT") {
        let after = std::env::var("ZTOP_DESKTOP_SNAPSHOT_AFTER")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        let fleet = fleet.clone();
        glib::timeout_add_local_once(Duration::from_secs(after), move || {
            match snapshot(&fleet.window, &path) {
                Ok(()) => eprintln!("ztop-desktop: wrote {path}"),
                Err(why) => eprintln!("ztop-desktop: snapshot failed: {why}"),
            }
            // What the picture shows, in words a script can check.
            eprintln!("ztop-desktop: tokens in {}", fleet.token_store_kind());
            for (name, state) in fleet.states() {
                eprintln!("ztop-desktop: host {name} {state}");
            }
            if let Some(detail) = fleet.detail.borrow().as_ref() {
                eprintln!("ztop-desktop: detail samples {}", detail.samples());
                for section in detail.visible_sections() {
                    eprintln!("ztop-desktop: detail shows {section}");
                }
            }
            if let Some(app) = fleet.window.application() {
                app.quit();
            }
        });
    }
}

/// Checks twice a second until the named host is live, then opens it.
fn open_detail_when_live(fleet: &Rc<Fleet>, name: String) {
    let fleet = Rc::downgrade(fleet);
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let Some(fleet) = fleet.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let live = fleet
            .states()
            .into_iter()
            .any(|(n, state)| n == name && state == "live");
        if !live {
            return glib::ControlFlow::Continue;
        }
        if let Some(record) = fleet.all_records().into_iter().find(|r| r.name == name) {
            fleet.open_detail(&record.id);
            if std::env::var("ZTOP_DESKTOP_SCROLL").as_deref() == Ok("processes") {
                // After the history and the first layout have landed.
                let fleet = Rc::downgrade(&fleet);
                glib::timeout_add_local_once(Duration::from_secs(3), move || {
                    if let Some(detail) = fleet.upgrade().and_then(|f| f.detail.borrow().clone()) {
                        detail.scroll_to_processes();
                    }
                });
            }
        }
        glib::ControlFlow::Break
    });
}

fn seed_alert(fleet: &Rc<Fleet>, spec: &str) {
    use ztop_desktop_core::alerts::{AlertKind, AlertRule};
    let parts: Vec<&str> = spec.split('|').collect();
    let [name, kind, threshold, seconds] = parts[..] else {
        eprintln!("ztop-desktop: an alert seed needs four fields: {spec}");
        return;
    };
    let kind = match serde_json::from_value::<AlertKind>(serde_json::Value::from(kind)) {
        Ok(kind) => kind,
        Err(_) => {
            eprintln!("ztop-desktop: unknown alert kind {kind}");
            return;
        }
    };
    let (Ok(threshold), Ok(seconds)) = (threshold.parse(), seconds.parse()) else {
        eprintln!("ztop-desktop: the alert seed's threshold and seconds are numbers: {spec}");
        return;
    };
    fleet.seed_alert(AlertRule::new(name, None, kind, threshold, seconds));
}

fn seed_hosts(fleet: &Rc<Fleet>, seed: &str) {
    let known: Vec<String> = fleet.records().into_iter().map(|r| r.name).collect();
    for entry in seed.split(';').filter(|e| !e.trim().is_empty()) {
        let parts: Vec<&str> = entry.split('|').collect();
        let (name, host, port, pin, code, kind) = match parts[..] {
            [name, host, port, pin, code] => (name, host, port, pin, code, ""),
            [name, host, port, pin, code, kind] => (name, host, port, pin, code, kind),
            _ => {
                eprintln!("ztop-desktop: a seed entry needs five or six fields: {entry}");
                continue;
            }
        };
        if known.iter().any(|k| k == name) {
            continue;
        }
        let form = NewHost {
            name: name.into(),
            host: host.into(),
            port: port.into(),
            pin: pin.into(),
            code: code.into(),
            hub: kind == "hub",
        };
        let record = match form.validate() {
            Ok(record) => record,
            Err(why) => {
                eprintln!("ztop-desktop: seed {name}: {why}");
                continue;
            }
        };
        let (fleet, code) = (fleet.clone(), code.to_string());
        glib::spawn_future_local(async move {
            let token = if code.is_empty() {
                Ok(None)
            } else {
                let record = record.clone();
                gio::spawn_blocking(move || agent::pair(&record, &code).map(Some))
                    .await
                    .unwrap_or_else(|_| Err("pairing crashed".into()))
            };
            let name = record.name.clone();
            let outcome = match token {
                Ok(token) => fleet.add_record(record, token).await,
                Err(why) => Err(why),
            };
            if let Err(why) = outcome {
                eprintln!("ztop-desktop: seed {name}: {why}");
            }
        });
    }
}

/// Draws the window as it is now into a PNG.
fn snapshot(window: &adw::ApplicationWindow, path: &str) -> Result<(), String> {
    let (width, height) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot.to_node().ok_or("the window drew nothing")?;
    let renderer = window.renderer().ok_or("the window has no renderer")?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    let texture = renderer.render_texture(&node, Some(&bounds));
    texture.save_to_png(path).map_err(|e| e.to_string())
}
