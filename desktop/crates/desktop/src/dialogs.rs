// SPDX-License-Identifier: MIT OR Apache-2.0
//! The add-host dialog, the pairing prompt and the remove confirmation.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use ztop_desktop_core::{NewHost, agent, discovery};

use crate::app::Fleet;

/// Agents advertising on the network (Bonjour). Choosing one fills in
/// the name, address and port; the fingerprint is still pasted from
/// the agent, because an advertisement proves nothing about who sent it.
fn nearby_group(
    name: &adw::EntryRow,
    host: &adw::EntryRow,
    port: &adw::EntryRow,
    pin: &adw::EntryRow,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("On This Network")
        .description("Agents started with --tls appear here.")
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("boxed-list");
    let looking = adw::ActionRow::builder()
        .title("Looking for agents…")
        .build();
    list.append(&looking);
    group.add(&list);

    let browser = match discovery::Browser::start() {
        Ok(browser) => Rc::new(std::cell::RefCell::new(browser)),
        Err(why) => {
            looking.set_title("Looking for agents is not possible here");
            looking.set_subtitle(&glib::markup_escape_text(&why));
            return group;
        }
    };
    let (name, host, port, pin) = (name.clone(), host.clone(), port.clone(), pin.clone());
    let weak_list = list.downgrade();
    // The browse lives as long as the dialog shows the list.
    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        let Some(list) = weak_list.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if !list.is_mapped() && list.root().is_none() {
            return glib::ControlFlow::Break;
        }
        if !browser.borrow_mut().poll() {
            return glib::ControlFlow::Continue;
        }
        while let Some(row) = list.row_at_index(0) {
            list.remove(&row);
        }
        let found = browser.borrow().found();
        if found.is_empty() {
            list.append(
                &adw::ActionRow::builder()
                    .title("Looking for agents…")
                    .build(),
            );
        }
        for agent in found {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&agent.name).as_str())
                .subtitle(
                    glib::markup_escape_text(&format!("{}:{}", agent.host, agent.port)).as_str(),
                )
                .activatable(agent.compatible())
                .build();
            if !agent.compatible() {
                row.set_subtitle("Speaks another protocol version: update the agent or this app.");
                row.set_sensitive(false);
            }
            let (name, host, port, pin) = (name.clone(), host.clone(), port.clone(), pin.clone());
            row.connect_activated(move |_| {
                if name.text().is_empty() {
                    name.set_text(&agent.name);
                }
                host.set_text(&agent.host);
                port.set_text(&agent.port.to_string());
                pin.grab_focus();
            });
            list.append(&row);
        }
        glib::ControlFlow::Continue
    });
    group
}

fn entry_row(title: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).build()
}

/// Address, port, fingerprint and code; pairs before saving when a
/// code is given, so a typo never leaves a half-added host behind.
pub fn add_host(fleet: &Rc<Fleet>) {
    let name = entry_row("Name (optional)");
    let host = entry_row("Address");
    let port = entry_row("Port");
    port.set_text(&agent::DEFAULT_PORT.to_string());
    port.set_input_purpose(gtk::InputPurpose::Digits);
    let pin = entry_row("TLS fingerprint (SHA256:…)");
    let code = entry_row("Pairing code");
    let hub = adw::SwitchRow::builder()
        .title("This is a hub")
        .subtitle(
            "A hub watches a fleet: each machine it watches gets a card, read through the hub.",
        )
        .build();
    {
        let port = port.clone();
        hub.connect_active_notify(move |hub| {
            // Follow the switch while the port is still a default.
            let text = port.text();
            if text == "7100" || text == "7101" || text.is_empty() {
                port.set_text(if hub.is_active() { "7101" } else { "7100" });
            }
        });
    }

    let connection = adw::PreferencesGroup::builder()
        .title("Connection")
        .description("Run ztop agent --tls --pair on the host: it prints the fingerprint and a pairing code. A plain agent on this machine needs neither.")
        .build();
    for row in [&name, &host, &port] {
        connection.add(row);
    }
    connection.add(&hub);
    let trust = adw::PreferencesGroup::builder().title("Trust").build();
    trust.add(&pin);
    trust.add(&code);

    let problem = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .visible(false)
        .build();
    problem.add_css_class("error");
    let spinner = gtk::Spinner::new();

    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    page.append(&nearby_group(&name, &host, &port, &pin));
    page.append(&connection);
    page.append(&trust);
    page.append(&problem);

    let cancel = gtk::Button::with_label("Cancel");
    let add = gtk::Button::with_label("Add");
    add.add_css_class("suggested-action");
    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&add);
    header.pack_end(&spinner);
    // The nearby list grows; the form scrolls instead of outgrowing
    // a small window.
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&page)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&scroller));

    let dialog = adw::Dialog::builder()
        .title("Add a Host")
        .content_width(460)
        .child(&view)
        .build();
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    let window = fleet.window.clone();
    let fleet = fleet.clone();
    let dialog_for_add = dialog.clone();
    add.connect_clicked(move |button| {
        let form = NewHost {
            name: name.text().into(),
            host: host.text().into(),
            port: port.text().into(),
            pin: pin.text().into(),
            code: code.text().into(),
            hub: hub.is_active(),
        };
        let record = match form.validate() {
            Ok(record) => record,
            Err(why) => {
                problem.set_label(&why);
                problem.set_visible(true);
                return;
            }
        };
        problem.set_visible(false);
        button.set_sensitive(false);
        spinner.start();
        let (fleet, dialog, button, spinner, problem) = (
            fleet.clone(),
            dialog_for_add.clone(),
            button.clone(),
            spinner.clone(),
            problem.clone(),
        );
        let code = form.code.trim().to_string();
        glib::spawn_future_local(async move {
            let token = if code.is_empty() {
                Ok(None)
            } else {
                let record = record.clone();
                gio::spawn_blocking(move || agent::pair(&record, &code).map(Some))
                    .await
                    .unwrap_or_else(|_| Err("pairing crashed".into()))
            };
            let outcome = match token {
                Ok(token) => fleet.add_record(record, token).await,
                Err(why) => Err(format!("Pairing failed: {why}")),
            };
            spinner.stop();
            button.set_sensitive(true);
            match outcome {
                Ok(()) => {
                    dialog.close();
                }
                Err(why) => {
                    problem.set_label(&why);
                    problem.set_visible(true);
                }
            }
        });
    });
    dialog.present(Some(&window));
}

/// Asks for the code of an agent that refused the saved token.
pub fn pair_host(fleet: &Rc<Fleet>, id: &str) {
    let Some(record) = fleet.records().into_iter().find(|r| r.id == id) else {
        return;
    };
    let entry = gtk::Entry::builder()
        .placeholder_text("Pairing code")
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::new(
        Some(&format!("Pair with {}", record.name)),
        Some("Start the agent with --pair (or restart it) and enter the code it prints."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("pair", "Pair");
    dialog.set_response_appearance("pair", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("pair"));
    dialog.set_close_response("cancel");
    dialog.set_extra_child(Some(&entry));
    let fleet_for_response = fleet.clone();
    dialog.connect_response(Some("pair"), move |_, _| {
        let code = entry.text().trim().to_string();
        let (fleet, record) = (fleet_for_response.clone(), record.clone());
        glib::spawn_future_local(async move {
            let id = record.id.clone();
            let token = gio::spawn_blocking(move || agent::pair(&record, &code))
                .await
                .unwrap_or_else(|_| Err("pairing crashed".into()));
            let outcome = match token {
                Ok(token) => fleet.paired(&id, token).await,
                Err(why) => Err(why),
            };
            if let Err(why) = outcome {
                fleet.toast(&format!("Pairing failed: {why}"));
            }
        });
    });
    dialog.present(Some(&fleet.window));
}

/// Removing forgets the host and deletes its token.
pub fn confirm_remove(fleet: &Rc<Fleet>, id: &str) {
    let Some(record) = fleet.records().into_iter().find(|r| r.id == id) else {
        return;
    };
    let dialog = adw::AlertDialog::new(
        Some(&format!("Remove {}?", record.name)),
        Some("zTop forgets this host and deletes its pairing token. The agent keeps running."),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Remove");
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    let (fleet_for_response, id) = (fleet.clone(), id.to_string());
    dialog.connect_response(Some("remove"), move |_, _| fleet_for_response.remove(&id));
    dialog.present(Some(&fleet.window));
}
