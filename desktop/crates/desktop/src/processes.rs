// SPDX-License-Identifier: MIT OR Apache-2.0
//! The detail page's process table: a search box, a sort menu, the
//! busiest rows, and a menu per row with the target's own signals.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};
use ztop_core::model::Process;
use ztop_desktop_core::processes::{Query, Signal, SortKey, outcome_text};
use ztop_desktop_core::{HostRecord, agent};

/// Rows shown at once; the search reaches the rest.
const ROWS: usize = 30;

struct Row {
    root: gtk::ListBoxRow,
    name: gtk::Label,
    detail: gtk::Label,
    cpu: gtk::Label,
    memory: gtk::Label,
    menu: gtk::MenuButton,
    pid: Cell<u32>,
    process_name: RefCell<String>,
}

pub struct ProcessTable {
    pub group: adw::PreferencesGroup,
    list: gtk::ListBox,
    count: gtk::Label,
    rows: Vec<Row>,
    query: RefCell<Query>,
    processes: RefCell<Vec<Process>>,
    catalog: RefCell<Vec<Signal>>,
    /// A row's menu is open: the table holds still so the row under
    /// the pointer is the row the signal goes to.
    holding: Cell<bool>,
    record: HostRecord,
    token: Option<String>,
    toasts: adw::ToastOverlay,
}

impl ProcessTable {
    pub fn new(record: &HostRecord, token: Option<String>, toasts: &adw::ToastOverlay) -> Rc<Self> {
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Name, user or PID")
            .hexpand(true)
            .build();
        // A recorded run may start with a search (see hooks.rs).
        let preset = std::env::var("ZTOP_DESKTOP_SEARCH").unwrap_or_default();
        let labels: Vec<&str> = SortKey::ALL.iter().map(|k| k.label()).collect();
        let sort = gtk::DropDown::from_strings(&labels);
        sort.set_tooltip_text(Some("Sort by"));
        let direction = gtk::ToggleButton::builder()
            .icon_name("view-sort-descending-symbolic")
            .tooltip_text("Largest first")
            .active(true)
            .build();
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        controls.append(&search);
        controls.append(&sort);
        controls.append(&direction);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        list.set_can_focus(false);
        let count = gtk::Label::builder().xalign(0.0).build();
        count.add_css_class("caption");
        count.add_css_class("dim-label");

        // Column names over the figures, aligned with the rows' own.
        let heading = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_start(12)
            .margin_end(6 + 34)
            .build();
        let spacer = gtk::Label::new(None);
        spacer.set_hexpand(true);
        heading.append(&spacer);
        for title in ["CPU", "Memory"] {
            let label = gtk::Label::builder()
                .label(title)
                .width_chars(7)
                .xalign(1.0)
                .build();
            label.add_css_class("caption-heading");
            label.add_css_class("dim-label");
            heading.append(&label);
        }

        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.append(&controls);
        column.append(&heading);
        column.append(&list);
        column.append(&count);
        let group = adw::PreferencesGroup::builder()
            .title("Processes")
            .visible(false)
            .build();
        group.add(&column);

        let rows: Vec<Row> = (0..ROWS).map(|_| Row::new()).collect();
        for row in &rows {
            list.append(&row.root);
        }
        let table = Rc::new(Self {
            group,
            list,
            count,
            rows,
            query: RefCell::default(),
            processes: RefCell::default(),
            catalog: RefCell::default(),
            holding: Cell::new(false),
            record: record.clone(),
            token,
            toasts: toasts.clone(),
        });

        let weak = Rc::downgrade(&table);
        search.connect_search_changed(move |entry| {
            if let Some(table) = weak.upgrade() {
                table.query.borrow_mut().search = entry.text().to_string();
                table.redraw();
            }
        });
        let weak = Rc::downgrade(&table);
        let direction_for_sort = direction.clone();
        sort.connect_selected_notify(move |sort| {
            let Some(table) = weak.upgrade() else { return };
            let key = SortKey::ALL[(sort.selected() as usize).min(SortKey::ALL.len() - 1)];
            table.query.borrow_mut().sort = key;
            // A new key starts in its natural direction.
            direction_for_sort.set_active(key.descending_by_default());
            table.redraw();
        });
        let weak = Rc::downgrade(&table);
        direction.connect_toggled(move |button| {
            let Some(table) = weak.upgrade() else { return };
            table.query.borrow_mut().descending = button.is_active();
            button.set_icon_name(if button.is_active() {
                "view-sort-descending-symbolic"
            } else {
                "view-sort-ascending-symbolic"
            });
            button.set_tooltip_text(Some(if button.is_active() {
                "Largest first"
            } else {
                "Smallest first"
            }));
            table.redraw();
        });
        for row in &table.rows {
            let popover = gtk::Popover::new();
            let weak = Rc::downgrade(&table);
            popover.connect_show(move |_| {
                if let Some(table) = weak.upgrade() {
                    table.holding.set(true);
                }
            });
            let weak = Rc::downgrade(&table);
            popover.connect_closed(move |_| {
                if let Some(table) = weak.upgrade() {
                    table.holding.set(false);
                    table.redraw();
                }
            });
            row.menu.set_popover(Some(&popover));
        }
        if !preset.is_empty() {
            search.set_text(&preset);
            table.query.borrow_mut().search = preset;
        }
        table.load_catalog();
        table
    }

    /// The newest process list from the poll.
    pub fn show(&self, processes: &[Process]) {
        *self.processes.borrow_mut() = processes.to_vec();
        self.group.set_visible(!processes.is_empty());
        self.redraw();
    }

    fn redraw(&self) {
        if self.holding.get() {
            return;
        }
        let processes = self.processes.borrow();
        let query = self.query.borrow();
        let table = query.table(&processes);
        for (i, row) in self.rows.iter().enumerate() {
            match table.get(i) {
                Some(process) => row.show(process),
                None => row.root.set_visible(false),
            }
        }
        self.list.set_visible(!table.is_empty());
        self.count.set_label(&match (table.len(), processes.len()) {
            (0, _) => "No process matches.".to_string(),
            (shown, all) if shown > ROWS => {
                format!("The first {ROWS} of {shown} matching, {all} in all.")
            }
            (shown, all) => format!("{shown} matching, {all} in all."),
        });
        let has_menu = !self.catalog.borrow().is_empty();
        for row in &self.rows {
            row.menu.set_visible(has_menu);
        }
    }

    /// Reads the target's signal catalog and builds each row's menu
    /// from it, so every signal goes out by the target's numbering.
    fn load_catalog(self: &Rc<Self>) {
        let (record, token) = (self.record.clone(), self.token.clone());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let catalog = gio::spawn_blocking(move || agent::signal_catalog(&record, token))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let Some(table) = weak.upgrade() else { return };
            for (index, row) in table.rows.iter().enumerate() {
                let Some(popover) = row.menu.popover() else {
                    continue;
                };
                let items = gtk::Box::new(gtk::Orientation::Vertical, 0);
                for signal in &catalog {
                    let button = gtk::Button::with_label(&signal.name);
                    button.add_css_class("flat");
                    if signal.needs_confirmation() {
                        button.add_css_class("destructive-action");
                    }
                    let (weak, signal, popover_ref) =
                        (Rc::downgrade(&table), signal.clone(), popover.clone());
                    button.connect_clicked(move |_| {
                        popover_ref.popdown();
                        if let Some(table) = weak.upgrade() {
                            table.chosen(index, &signal);
                        }
                    });
                    items.append(&button);
                }
                popover.set_child(Some(&items));
            }
            *table.catalog.borrow_mut() = catalog;
            table.redraw();
        });
    }

    /// A signal was picked from row `index`'s menu.
    fn chosen(self: &Rc<Self>, index: usize, signal: &Signal) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let (pid, name) = (row.pid.get(), row.process_name.borrow().clone());
        if !signal.needs_confirmation() {
            self.send(pid, name, signal.clone());
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some(&format!("Kill {name}?")),
            Some(&format!(
                "KILL ends process {pid} at once, without letting it save or clean up. TERM asks it to stop."
            )),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("kill", "Kill")]);
        dialog.set_response_appearance("kill", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let (weak, signal): (Weak<Self>, Signal) = (Rc::downgrade(self), signal.clone());
        dialog.connect_response(None, move |_, response| {
            if response == "kill" {
                if let Some(table) = weak.upgrade() {
                    table.send(pid, name.clone(), signal.clone());
                }
            }
        });
        dialog.present(Some(&self.group));
    }

    fn send(self: &Rc<Self>, pid: u32, name: String, signal: Signal) {
        let (record, token) = (self.record.clone(), self.token.clone());
        let toasts = self.toasts.clone();
        let number = signal.number;
        glib::spawn_future_local(async move {
            let results =
                gio::spawn_blocking(move || agent::send_signal(&record, token, &[pid], number))
                    .await
                    .unwrap_or_else(|_| vec![(pid, Err("the request crashed".into()))]);
            let result = results
                .into_iter()
                .find(|(p, _)| *p == pid)
                .map_or(Err("the agent did not answer for it".into()), |(_, r)| r);
            toasts.add_toast(adw::Toast::new(&outcome_text(
                &signal.name,
                &name,
                pid,
                &result,
            )));
        });
    }
}

impl Row {
    fn new() -> Self {
        let name = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let detail = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        detail.add_css_class("caption");
        detail.add_css_class("dim-label");
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        titles.append(&name);
        titles.append(&detail);
        let figure = || {
            let label = gtk::Label::builder().width_chars(7).xalign(1.0).build();
            label.add_css_class("numeric");
            label
        };
        let (cpu, memory) = (figure(), figure());
        cpu.set_tooltip_text(Some("CPU"));
        memory.set_tooltip_text(Some("Memory"));
        let menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Send a signal")
            .valign(gtk::Align::Center)
            .build();
        menu.add_css_class("flat");
        let line = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(6)
            .build();
        line.append(&titles);
        line.append(&cpu);
        line.append(&memory);
        line.append(&menu);
        let root = gtk::ListBoxRow::builder()
            .child(&line)
            .activatable(false)
            .visible(false)
            .build();
        Self {
            root,
            name,
            detail,
            cpu,
            memory,
            menu,
            pid: Cell::new(0),
            process_name: RefCell::default(),
        }
    }

    fn show(&self, process: &Process) {
        self.root.set_visible(true);
        self.name.set_label(&process.name);
        self.detail.set_label(&format!(
            "{} · {} · pid {}",
            process.user, process.state, process.pid
        ));
        self.cpu.set_label(&format!("{:.1}%", process.cpu_percent));
        self.memory
            .set_label(&format!("{:.1}%", process.mem_percent));
        self.pid.set(process.pid);
        *self.process_name.borrow_mut() = process.name.clone();
    }
}
