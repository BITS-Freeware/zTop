// SPDX-License-Identifier: MIT OR Apache-2.0
//! The alerts page (rules, which hosts each is firing for, add and
//! delete) and the desktop notifications fired and withdrawn as hosts
//! are read.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use ztop_desktop_core::alerts::{AlertEvent, AlertKind, AlertRule};

use crate::app::Fleet;

/// Shows an alert event as a desktop notification and a toast, and
/// reports it on stderr for recorded runs.
pub fn announce(fleet: &Fleet, event: &AlertEvent, host_name: &str) {
    let Some(application) = fleet.window.application() else {
        return;
    };
    match event {
        AlertEvent::Fired { rule, host, name } => {
            let title = format!("{name} on {host_name}");
            let body = fleet
                .alert_rules()
                .iter()
                .find(|r| &r.id == rule)
                .map(AlertRule::describe)
                .unwrap_or_default();
            let notification = gio::Notification::new(&title);
            notification.set_body(Some(&body));
            notification.set_priority(gio::NotificationPriority::High);
            application.send_notification(Some(&notification_id(rule, host)), &notification);
            fleet.toast(&format!("{title}: {body}"));
            eprintln!("ztop-desktop: alert fired {name} on {host_name}");
        }
        AlertEvent::Cleared { rule, host, name } => {
            application.withdraw_notification(&notification_id(rule, host));
            fleet.toast(&format!("{name} on {host_name} cleared"));
            eprintln!("ztop-desktop: alert cleared {name} on {host_name}");
        }
    }
}

fn notification_id(rule: &str, host: &str) -> String {
    format!("alert-{rule}-{host}")
}

/// The alerts page, rebuilt from the fleet's rules each time it is
/// shown or changed.
pub struct AlertsPage {
    pub page: adw::NavigationPage,
    list: gtk::ListBox,
    empty: adw::StatusPage,
}

impl AlertsPage {
    pub fn new(fleet: &Rc<Fleet>) -> Rc<Self> {
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.set_tooltip_text(Some("Add a rule"));
        let header = adw::HeaderBar::new();
        header.pack_end(&add);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .build();
        list.add_css_class("boxed-list");
        let empty = adw::StatusPage::builder()
            .icon_name("alarm-symbolic")
            .title("No Alert Rules")
            .description("A rule watches one host or all of them, and notifies you when CPU, memory, load or disk stays above a threshold, or a host stays unreachable.")
            .build();
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        column.append(&list);
        column.append(&empty);
        let clamp = adw::Clamp::builder()
            .maximum_size(720)
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
            .title("Alerts")
            .tag("alerts")
            .child(&view)
            .build();
        let alerts = Rc::new(Self { page, list, empty });
        {
            let (fleet, weak) = (fleet.clone(), Rc::downgrade(&alerts));
            add.connect_clicked(move |_| {
                if let Some(alerts) = weak.upgrade() {
                    add_rule_dialog(&fleet, &alerts);
                }
            });
        }
        alerts.refresh(fleet);
        alerts
    }

    /// Rows for the rules as they are now, with the hosts each fires for.
    pub fn refresh(self: &Rc<Self>, fleet: &Rc<Fleet>) {
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let rules = fleet.alert_rules();
        self.list.set_visible(!rules.is_empty());
        self.empty.set_visible(rules.is_empty());
        for rule in rules {
            let firing = fleet.firing_for(&rule.id);
            let subtitle = if firing.is_empty() {
                rule.describe()
            } else {
                format!("{} · firing for {}", rule.describe(), firing.join(", "))
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&rule.name).as_str())
                .subtitle(glib::markup_escape_text(&subtitle).as_str())
                .build();
            if !firing.is_empty() {
                let badge = gtk::Label::new(Some("firing"));
                badge.add_css_class("state-pill");
                badge.add_css_class("unreachable");
                badge.set_valign(gtk::Align::Center);
                row.add_suffix(&badge);
            }
            let delete = gtk::Button::from_icon_name("user-trash-symbolic");
            delete.add_css_class("flat");
            delete.set_valign(gtk::Align::Center);
            delete.set_tooltip_text(Some("Delete this rule"));
            let (fleet, weak, id) = (fleet.clone(), Rc::downgrade(self), rule.id.clone());
            delete.connect_clicked(move |_| {
                fleet.remove_alert_rule(&id);
                if let Some(alerts) = weak.upgrade() {
                    alerts.refresh(&fleet);
                }
            });
            row.add_suffix(&delete);
            self.list.append(&row);
        }
    }
}

/// Name, what to watch, where, how high and for how long.
fn add_rule_dialog(fleet: &Rc<Fleet>, alerts: &Rc<AlertsPage>) {
    let name = adw::EntryRow::builder().title("Name").build();
    let kinds: Vec<&str> = AlertKind::ALL.iter().map(|k| k.label()).collect();
    let kind = adw::ComboRow::builder()
        .title("Watch")
        .model(&gtk::StringList::new(&kinds))
        .build();
    let records = fleet.all_records();
    let mut hosts: Vec<String> = vec!["Every host".into()];
    hosts.extend(records.iter().filter(|r| !r.hub).map(|r| r.name.clone()));
    let host_refs: Vec<&str> = hosts.iter().map(String::as_str).collect();
    let host = adw::ComboRow::builder()
        .title("Host")
        .model(&gtk::StringList::new(&host_refs))
        .build();
    let threshold = adw::SpinRow::with_range(0.0, 1000.0, 1.0);
    threshold.set_title("Above");
    threshold.set_subtitle("Percent for CPU, memory and disk; the 1-minute average for load");
    threshold.set_value(90.0);
    threshold.set_digits(1);
    let minutes = adw::SpinRow::with_range(0.0, 1440.0, 1.0);
    minutes.set_title("For minutes");
    minutes.set_value(5.0);
    {
        let threshold = threshold.clone();
        kind.connect_selected_notify(move |kind| {
            let chosen = AlertKind::ALL[(kind.selected() as usize).min(AlertKind::ALL.len() - 1)];
            threshold.set_sensitive(chosen.uses_threshold());
        });
    }
    let group = adw::PreferencesGroup::new();
    for row in [
        name.upcast_ref::<gtk::Widget>(),
        kind.upcast_ref(),
        host.upcast_ref(),
        threshold.upcast_ref(),
        minutes.upcast_ref(),
    ] {
        group.add(row);
    }
    let dialog = adw::AlertDialog::new(Some("Add an Alert Rule"), None);
    dialog.set_extra_child(Some(&group));
    dialog.add_responses(&[("cancel", "Cancel"), ("add", "Add")]);
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("cancel");
    let (fleet_for_add, alerts) = (fleet.clone(), alerts.clone());
    dialog.connect_response(Some("add"), move |_, _| {
        let chosen = AlertKind::ALL[(kind.selected() as usize).min(AlertKind::ALL.len() - 1)];
        let host_id = match host.selected() as usize {
            0 => None,
            i => records
                .iter()
                .filter(|r| !r.hub)
                .nth(i - 1)
                .map(|r| r.id.clone()),
        };
        let title = match name.text().trim() {
            "" => format!("{} alert", chosen.label()),
            text => text.to_string(),
        };
        let rule = AlertRule::new(
            &title,
            host_id,
            chosen,
            threshold.value(),
            (minutes.value() * 60.0).round() as u64,
        );
        fleet_for_add.add_alert_rule(rule);
        alerts.refresh(&fleet_for_add);
    });
    dialog.present(Some(&fleet.window));
}
