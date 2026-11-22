// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window and the fleet's state. Everything here runs on the GTK
//! main thread; reads, pairing and token-store calls go to worker
//! threads through `gio::spawn_blocking` and come back as futures.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use adw::prelude::*;
use gtk::{gio, glib};
use ztop_desktop_core::freshness::POLL_INTERVAL;
use ztop_desktop_core::{FileTokenStore, HostLive, HostRecord, HostRegistry, TokenStore, agent};

use crate::alerts::AlertsPage;
use crate::card::Card;
use crate::detail::Detail;
use crate::secret::SecretServiceStore;
use ztop_desktop_core::alerts::{AlertBook, AlertEngine, AlertRule};

const CSS: &str = r#"
.state-pill { border-radius: 999px; padding: 2px 10px; font-size: smaller; font-weight: bold; }
.state-pill.live { background: alpha(@success_color, 0.18); color: @success_color; }
.state-pill.stale { background: alpha(@warning_color, 0.2); color: @warning_color; }
.state-pill.unreachable { background: alpha(@error_color, 0.18); color: @error_color; }
.state-pill.pairing { background: alpha(@accent_color, 0.18); color: @accent_color; }
.state-pill.connecting { background: alpha(currentColor, 0.1); }
flowboxchild { padding: 0; }
"#;

pub fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// The fleet: saved records, their tokens and live state, and the
/// widgets that show them.
pub struct Fleet {
    pub window: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    nav: adw::NavigationView,
    stack: gtk::Stack,
    flow: gtk::FlowBox,
    banner: adw::Banner,
    registry: RefCell<Option<HostRegistry>>,
    pub store: RefCell<Option<Arc<dyn TokenStore>>>,
    /// Tokens read once at startup and kept here, so a poll never
    /// waits on the keyring.
    tokens: RefCell<HashMap<String, String>>,
    live: RefCell<HashMap<String, HostLive>>,
    cards: RefCell<HashMap<String, Card>>,
    in_flight: RefCell<HashSet<String>>,
    /// Each hub's machines (by hub id), read through it; never saved.
    members: RefCell<HashMap<String, Vec<HostRecord>>>,
    /// The saved alert rules, and the engine that fires them.
    alert_book: RefCell<Option<AlertBook>>,
    engine: RefCell<AlertEngine>,
    alerts_page: RefCell<Option<Rc<AlertsPage>>>,
    /// Each hub's own state, from its host list.
    hub_states: RefCell<HashMap<String, ztop_desktop_core::HostState>>,
    /// The open detail page, if any; it follows its host's polls.
    pub detail: RefCell<Option<Rc<Detail>>>,
}

pub fn activate(application: &adw::Application) {
    if let Some(window) = application.active_window() {
        window.present();
        return;
    }
    let fleet = Fleet::build(application);
    fleet.window.present();
    let fleet_for_load = fleet.clone();
    glib::spawn_future_local(async move { fleet_for_load.load().await });
}

impl Fleet {
    fn build(application: &adw::Application) -> Rc<Self> {
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.set_tooltip_text(Some("Add a host"));
        let bell = gtk::Button::from_icon_name("alarm-symbolic");
        bell.set_tooltip_text(Some("Alert rules"));
        let header = adw::HeaderBar::new();
        header.pack_start(&add);
        header.pack_end(&bell);

        let banner = adw::Banner::new(
            "No desktop keyring is running, so pairing tokens are kept in a private file instead.",
        );
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(4)
            .row_spacing(12)
            .column_spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .valign(gtk::Align::Start)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&flow)
            .build();

        let add_first = gtk::Button::builder()
            .label("Add a Host")
            .halign(gtk::Align::Center)
            .build();
        add_first.add_css_class("pill");
        add_first.add_css_class("suggested-action");
        let empty = adw::StatusPage::builder()
            .icon_name("network-server-symbolic")
            .title("No Hosts Yet")
            .description("Add a machine running ztop agent. A remote agent needs its TLS fingerprint and a pairing code, both printed when it starts with --tls --pair.")
            .child(&add_first)
            .build();
        let loading = adw::StatusPage::builder().title("Loading…").build();

        let stack = gtk::Stack::new();
        stack.add_named(&loading, Some("loading"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scroller, Some("fleet"));
        stack.set_visible_child_name("loading");

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.add_top_bar(&banner);
        view.set_content(Some(&stack));
        let fleet_page = adw::NavigationPage::builder()
            .title("zTop")
            .tag("fleet")
            .child(&view)
            .build();
        let nav = adw::NavigationView::new();
        nav.add(&fleet_page);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&nav));

        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("zTop")
            .default_width(1000)
            .default_height(640)
            .width_request(360)
            .height_request(300)
            .content(&toasts)
            .build();

        let fleet = Rc::new(Self {
            window,
            toasts,
            nav,
            stack,
            flow,
            banner,
            registry: RefCell::new(None),
            store: RefCell::new(None),
            tokens: RefCell::new(HashMap::new()),
            live: RefCell::new(HashMap::new()),
            cards: RefCell::new(HashMap::new()),
            in_flight: RefCell::new(HashSet::new()),
            detail: RefCell::new(None),
            members: RefCell::new(HashMap::new()),
            hub_states: RefCell::new(HashMap::new()),
            alert_book: RefCell::new(None),
            engine: RefCell::new(AlertEngine::default()),
            alerts_page: RefCell::new(None),
        });
        {
            let flow = fleet.flow.clone();
            let fleet = Rc::downgrade(&fleet);
            flow.connect_child_activated(move |_, child| {
                if let Some(fleet) = fleet.upgrade() {
                    fleet.open_detail(&child.widget_name());
                }
            });
        }
        {
            let fleet = fleet.clone();
            bell.connect_clicked(move |_| fleet.open_alerts());
        }
        for button in [&add, &add_first] {
            let fleet = fleet.clone();
            button.connect_clicked(move |_| crate::dialogs::add_host(&fleet));
        }
        fleet
    }

    /// Picks the token store, loads the saved hosts and their tokens off
    /// the UI thread, then starts polling.
    async fn load(self: Rc<Self>) {
        let Some(dir) = ztop_desktop_core::config_dir() else {
            self.toast("No home directory: zTop cannot save hosts.");
            return;
        };
        let loaded = gio::spawn_blocking(move || {
            let store: Arc<dyn TokenStore> = match SecretServiceStore::probe() {
                Some(store) => Arc::new(store),
                None => Arc::new(FileTokenStore::new(&dir)),
            };
            let registry = HostRegistry::load(&dir).map_err(|e| e.to_string())?;
            let book = AlertBook::load(&dir).map_err(|e| format!("alerts.json: {e}"))?;
            let tokens: HashMap<String, String> = registry
                .records
                .iter()
                .filter_map(|r| store.load(&r.id).map(|t| (r.id.clone(), t)))
                .collect();
            Ok::<_, String>((store, registry, tokens, book))
        })
        .await
        .unwrap_or_else(|_| Err("loading the saved hosts failed".into()));

        let (store, registry, tokens, book) = match loaded {
            Ok(loaded) => loaded,
            Err(why) => {
                self.stack.set_visible_child_name("empty");
                self.toast(&format!("Could not read the saved hosts: {why}"));
                return;
            }
        };
        self.banner.set_revealed(store.kind() == "a private file");
        *self.store.borrow_mut() = Some(store);
        *self.alert_book.borrow_mut() = Some(book);
        *self.tokens.borrow_mut() = tokens;
        let records = registry.records.clone();
        *self.registry.borrow_mut() = Some(registry);
        for record in &records {
            self.add_card(record);
        }
        self.refresh_stack();

        self.poll_all();
        let fleet = Rc::downgrade(&self);
        glib::timeout_add_local(POLL_INTERVAL, move || match fleet.upgrade() {
            Some(fleet) => {
                fleet.poll_all();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        crate::hooks::run(&self);
    }

    pub fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn refresh_stack(&self) {
        let empty = self.cards.borrow().is_empty();
        self.stack
            .set_visible_child_name(if empty { "empty" } else { "fleet" });
    }

    /// Each saved host's name and the word its card shows now.
    pub fn states(&self) -> Vec<(String, &'static str)> {
        let live = self.live.borrow();
        let hub_states = self.hub_states.borrow();
        self.all_records()
            .into_iter()
            .map(|r| {
                let state = hub_states.get(&r.id).copied().unwrap_or_else(|| {
                    live.get(&r.id)
                        .map_or(ztop_desktop_core::HostState::Connecting, |l| l.state)
                });
                (r.name, state.label())
            })
            .collect()
    }

    pub fn token_store_kind(&self) -> &'static str {
        self.store.borrow().as_ref().map_or("none", |s| s.kind())
    }

    pub fn records(&self) -> Vec<HostRecord> {
        self.registry
            .borrow()
            .as_ref()
            .map(|r| r.records.clone())
            .unwrap_or_default()
    }

    /// The saved records and every hub's machines.
    pub fn all_records(&self) -> Vec<HostRecord> {
        let members = self.members.borrow();
        let mut all = Vec::new();
        for record in self.records() {
            let machines = members.get(&record.id).cloned().unwrap_or_default();
            all.push(record);
            all.extend(machines);
        }
        all
    }

    fn add_card(self: &Rc<Self>, record: &HostRecord) {
        let card = Card::new(record);
        let id = record.id.clone();
        // The flow box names each child by its host, so activating a
        // card finds its record.
        card.root.set_widget_name(&id);
        {
            let fleet = self.clone();
            let id = record.token_id().to_string();
            card.pair
                .connect_clicked(move |_| crate::dialogs::pair_host(&fleet, &id));
        }
        {
            let fleet = self.clone();
            let id = id.clone();
            card.remove
                .connect_clicked(move |_| crate::dialogs::confirm_remove(&fleet, &id));
        }
        // A machine read through a hub goes when the hub drops it.
        card.remove.set_visible(record.via.is_none());
        // A hub's machines sit right after its card.
        let after_hub = record.via.as_ref().and_then(|(hub, _)| {
            let cards = self.cards.borrow();
            let mut last = cards.get(hub)?.root.index();
            for (id, other) in cards.iter() {
                if id.starts_with(&format!("{hub}/")) {
                    last = last.max(other.root.index());
                }
            }
            Some(last + 1)
        });
        match after_hub {
            Some(position) => self.flow.insert(&card.root, position),
            None => self.flow.append(&card.root),
        }
        self.cards.borrow_mut().insert(id, card);
    }

    /// Saves a new record (and its token, when it was paired), shows
    /// its card and reads it at once.
    pub async fn add_record(
        self: &Rc<Self>,
        record: HostRecord,
        token: Option<String>,
    ) -> Result<(), String> {
        if let Some(token) = &token {
            self.save_token(&record.id, token).await?;
        }
        {
            let mut registry = self.registry.borrow_mut();
            let registry = registry.as_mut().ok_or("the saved hosts are not loaded")?;
            registry.add(record.clone()).map_err(|e| e.to_string())?;
        }
        self.add_card(&record);
        self.refresh_stack();
        self.poll(&record);
        Ok(())
    }

    /// Stores a token in the token store and the in-memory cache.
    pub async fn save_token(&self, id: &str, token: &str) -> Result<(), String> {
        let store = self.store.borrow().clone().ok_or("no token store")?;
        let (id_owned, token_owned) = (id.to_string(), token.to_string());
        gio::spawn_blocking(move || store.save(&id_owned, &token_owned))
            .await
            .unwrap_or_else(|_| Err("saving the token failed".into()))?;
        self.tokens
            .borrow_mut()
            .insert(id.to_string(), token.to_string());
        Ok(())
    }

    /// After pairing an existing host: keep the token and read now.
    pub async fn paired(self: &Rc<Self>, id: &str, token: String) -> Result<(), String> {
        self.save_token(id, &token).await?;
        if let Some(record) = self.records().into_iter().find(|r| r.id == id) {
            self.poll(&record);
        }
        Ok(())
    }

    pub fn remove(self: &Rc<Self>, id: &str) {
        if let Some(registry) = self.registry.borrow_mut().as_mut()
            && let Err(why) = registry.remove(id)
        {
            self.toast(&format!("Could not save the change: {why}"));
            return;
        }
        if let Some(card) = self.cards.borrow_mut().remove(id) {
            self.flow.remove(&card.root);
        }
        let mut gone = vec![id.to_string()];
        for machine in self.members.borrow_mut().remove(id).unwrap_or_default() {
            if let Some(card) = self.cards.borrow_mut().remove(&machine.id) {
                self.flow.remove(&card.root);
            }
            self.live.borrow_mut().remove(&machine.id);
            gone.push(machine.id);
        }
        // Its alerts stop firing and their notifications go with it.
        let rules = self.alert_rules();
        let cleared: Vec<_> = gone
            .iter()
            .flat_map(|host| self.engine.borrow_mut().forget(&rules, host))
            .collect();
        crate::alerts::withdraw(self, &cleared);
        if !cleared.is_empty() {
            let page = self.alerts_page.borrow().clone();
            if let Some(page) = page {
                page.refresh(self);
            }
        }
        self.hub_states.borrow_mut().remove(id);
        self.live.borrow_mut().remove(id);
        self.tokens.borrow_mut().remove(id);
        if let Some(store) = self.store.borrow().clone() {
            let id = id.to_string();
            gio::spawn_blocking(move || store.delete(&id));
        }
        self.refresh_stack();
    }

    /// Pushes the detail page for host `id` and reads the agent's
    /// history for it on a worker thread.
    pub fn open_detail(self: &Rc<Self>, id: &str) {
        let Some(record) = self
            .all_records()
            .into_iter()
            .find(|r| r.id == id && !r.hub)
        else {
            return;
        };
        let token = self.tokens.borrow().get(record.token_id()).cloned();
        let detail = Detail::new(&record, token.clone(), &self.toasts);
        {
            let fleet = Rc::downgrade(self);
            let id = record.id.clone();
            detail.page.connect_hidden(move |_| {
                if let Some(fleet) = fleet.upgrade() {
                    let mut open = fleet.detail.borrow_mut();
                    if open.as_ref().is_some_and(|d| d.id == id) {
                        *open = None;
                    }
                }
            });
        }
        // The newest polled sample shows at once; the history fills in
        // behind it.
        if let Some(sample) = self.live.borrow().get(id).and_then(|l| l.sample.clone()) {
            detail.push(sample);
        }
        *self.detail.borrow_mut() = Some(detail.clone());
        self.nav.push(&detail.page);
        glib::spawn_future_local(async move {
            let seconds = ztop_desktop_core::history::WINDOW_SECONDS;
            let result = gio::spawn_blocking(move || agent::history(&record, token, seconds))
                .await
                .unwrap_or_else(|_| Err(agent::ReadError::Failed("the read crashed".into())))
                .map_err(|e| e.to_string());
            detail.seed(result);
        });
    }

    fn poll_all(self: &Rc<Self>) {
        for record in self.records() {
            self.poll(&record);
        }
    }

    /// Reads one host on a worker thread, unless a read is already out:
    /// a slow host never piles up threads or holds up the others.
    fn poll(self: &Rc<Self>, record: &HostRecord) {
        if record.hub {
            self.poll_hub(record);
            return;
        }
        if !self.in_flight.borrow_mut().insert(record.id.clone()) {
            return;
        }
        let token = self.tokens.borrow().get(record.token_id()).cloned();
        let record = record.clone();
        let record_name = record.name.clone();
        let fleet = self.clone();
        glib::spawn_future_local(async move {
            let id = record.id.clone();
            let result = gio::spawn_blocking(move || agent::read(&record, token))
                .await
                .unwrap_or_else(|_| Err(agent::ReadError::Failed("the read crashed".into())));
            fleet.in_flight.borrow_mut().remove(&id);
            // A host removed while its read was out stays removed.
            let cards = fleet.cards.borrow();
            let Some(card) = cards.get(&id) else { return };
            let mut live = fleet.live.borrow_mut();
            let entry_id = id.clone();
            let name = record_name.clone();
            let entry = live.entry(id).or_default();
            entry.apply(result, Instant::now());
            card.show(entry);
            let fresh = entry.state == ztop_desktop_core::HostState::Live;
            if let (true, Some(sample), Some(detail)) =
                (fresh, entry.sample.clone(), fleet.detail.borrow().clone())
                && detail.id == entry_id
            {
                detail.push(sample);
            }
            // Alerts read the same result: metrics only from a live
            // sample, and unreachable once the freshness window passed.
            let state = entry.state;
            let sample = (state == ztop_desktop_core::HostState::Live)
                .then(|| entry.sample.clone())
                .flatten();
            drop(live);
            drop(cards);
            fleet.evaluate_alerts(&entry_id, &name, sample.as_ref(), state);
        });
    }
}

impl Fleet {
    /// Reads a hub's host list: its own card shows how many machines
    /// it watches, each machine gets (or loses) a card of its own, and
    /// every machine is then read through the hub.
    fn poll_hub(self: &Rc<Self>, hub: &HostRecord) {
        if !self.in_flight.borrow_mut().insert(hub.id.clone()) {
            return;
        }
        let token = self.tokens.borrow().get(&hub.id).cloned();
        let fleet = self.clone();
        let hub = hub.clone();
        glib::spawn_future_local(async move {
            let for_read = hub.clone();
            let result = gio::spawn_blocking(move || agent::hub_hosts(&for_read, token))
                .await
                .unwrap_or_else(|_| Err(agent::ReadError::Failed("the read crashed".into())));
            fleet.in_flight.borrow_mut().remove(&hub.id);
            if !fleet.cards.borrow().contains_key(&hub.id) {
                return;
            }
            use ztop_desktop_core::HostState;
            let (state, summary, error) = match &result {
                Ok(hosts) => {
                    let live = hosts.iter().filter(|h| h.state == "live").count();
                    let summary = match hosts.len() {
                        0 => "Watching no machines yet".to_string(),
                        1 => format!("Watching 1 machine, {live} live"),
                        n => format!("Watching {n} machines, {live} live"),
                    };
                    (HostState::Live, summary, None)
                }
                Err(agent::ReadError::NeedsPairing) => (
                    HostState::NeedsPairing,
                    String::new(),
                    Some(agent::ReadError::NeedsPairing.to_string()),
                ),
                Err(why) => (HostState::Unreachable, String::new(), Some(why.to_string())),
            };
            fleet.hub_states.borrow_mut().insert(hub.id.clone(), state);
            if let Some(card) = fleet.cards.borrow().get(&hub.id) {
                card.show_hub(state, &summary, error.as_deref());
            }
            let Ok(hosts) = result else { return };

            // Machines the hub no longer lists lose their cards.
            let names: Vec<String> = hosts.into_iter().map(|h| h.name).collect();
            let known = fleet
                .members
                .borrow()
                .get(&hub.id)
                .cloned()
                .unwrap_or_default();
            for gone in known.iter().filter(|m| !names.contains(&m.name)) {
                if let Some(card) = fleet.cards.borrow_mut().remove(&gone.id) {
                    fleet.flow.remove(&card.root);
                }
                fleet.live.borrow_mut().remove(&gone.id);
            }
            let machines: Vec<HostRecord> = names.iter().map(|n| hub.machine(n)).collect();
            for machine in &machines {
                if !fleet.cards.borrow().contains_key(&machine.id) {
                    fleet.add_card(machine);
                }
            }
            fleet
                .members
                .borrow_mut()
                .insert(hub.id.clone(), machines.clone());
            fleet.refresh_stack();
            for machine in &machines {
                fleet.poll(machine);
            }
        });
    }
}

impl Fleet {
    pub fn alert_rules(&self) -> Vec<AlertRule> {
        self.alert_book
            .borrow()
            .as_ref()
            .map(|b| b.rules.clone())
            .unwrap_or_default()
    }

    /// The names of the hosts a rule is firing for now.
    pub fn firing_for(&self, rule: &str) -> Vec<String> {
        let all = self.all_records();
        self.engine
            .borrow()
            .firing(rule)
            .into_iter()
            .map(|id| {
                all.iter()
                    .find(|r| r.id == id)
                    .map_or(id, |r| r.name.clone())
            })
            .collect()
    }

    pub fn add_alert_rule(&self, rule: AlertRule) {
        let mut book = self.alert_book.borrow_mut();
        let Some(book) = book.as_mut() else { return };
        book.rules.push(rule);
        if let Err(why) = book.save() {
            self.toast(&format!("Could not save the rule: {why}"));
        }
    }

    pub fn remove_alert_rule(&self, id: &str) {
        let mut book = self.alert_book.borrow_mut();
        let Some(book) = book.as_mut() else { return };
        book.rules.retain(|r| r.id != id);
        if let Err(why) = book.save() {
            self.toast(&format!("Could not save the change: {why}"));
        }
    }

    pub fn open_alerts(self: &Rc<Self>) {
        let page = AlertsPage::new(self);
        self.nav.push(&page.page);
        *self.alerts_page.borrow_mut() = Some(page);
    }

    fn evaluate_alerts(
        self: &Rc<Self>,
        id: &str,
        name: &str,
        sample: Option<&ztop_core::model::Sample>,
        state: ztop_desktop_core::HostState,
    ) {
        let rules = self.alert_rules();
        if rules.is_empty() {
            return;
        }
        let unreachable = state == ztop_desktop_core::HostState::Unreachable;
        let events =
            self.engine
                .borrow_mut()
                .evaluate(&rules, id, sample, unreachable, Instant::now());
        for event in &events {
            crate::alerts::announce(self, event, name);
        }
        if !events.is_empty() {
            let page = self.alerts_page.borrow().clone();
            if let Some(page) = page {
                page.refresh(self);
            }
        }
    }

    /// Adds a rule for every host (recorded runs), unless rules exist.
    pub fn seed_alert(&self, rule: AlertRule) {
        if self.alert_rules().is_empty() {
            self.add_alert_rule(rule);
        }
    }
}
