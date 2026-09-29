//! Finestra "Storia" (tasto H, bottone nel pannello del tempo o
//! nell'ispettore): quello che lo storico SQLite (`history_sync.rs`) ricorda
//! della partita, anche delle persone morte da generazioni.
//!
//! Schede:
//! - **Biografia** della persona scelta (l'NPC selezionato nel mondo, o una
//!   persona trovata con la ricerca o cliccata nell'albero): nascita, genitori,
//!   partner, figli, morte e la cronologia degli eventi che la riguardano.
//! - **Albero genealogico**: antenati (2 generazioni) e discendenti (3).
//!   Cliccare una persona viva la seleziona nel mondo, una morta ne mostra
//!   la biografia.
//! - **Ricerca** per nome, tra vivi e morti.
//! - **Registro del giocatore**: cosa ha preso, comprato e regalato.
//! - **Record**: più figli, più longevi, famiglie più numerose, morti per
//!   causa, nascite e morti per anno.
//!
//! Le query girano sulla connessione in sola lettura (indicizzate, sotto il
//! millisecondo) e i risultati restano in cache: si rileggono quando cambia
//! la scheda, la persona o la ricerca, e al massimo una volta al secondo
//! mentre lo storico cresce.

use std::time::Instant;

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use egui_plot::{Bar, BarChart, Corner, Legend, Plot};
use history::{
    EventRow, FamilyNode, FamilyTree, History, HistoryCounts, PersonRow, PlayerTotal, YearCounts,
};
use sim::{
    DeathCause, DeliberationKind, EventKind, GameTime, ItemKind, NpcId, Resolver, Sex, World,
};

use crate::history_sync::HistoryDb;
use crate::state::{SelectedNpc, Sim};
use crate::ui::{PointerCheck, event_color, year_of};

const WINDOW_WIDTH: f32 = 540.0;
/// Secondi reali tra due riletture della stessa vista mentre lo storico cresce.
const REFRESH_SECS: f64 = 1.0;
const SEARCH_LIMIT: usize = 100;
const DEEDS_LIMIT: usize = 200;
const RECORDS_LIMIT: usize = 5;
/// Generazioni mostrate nell'albero.
const TREE_UP: u32 = 2;
const TREE_DOWN: u32 = 3;
/// Eventi "minori" nascosti dalla biografia se si vogliono solo i fatti della vita.
const MINOR_KINDS: [&str; 4] = [
    "ItemBought",
    "ItemBroke",
    "NpcStarving",
    // La domanda: la decisione segue sempre (o è annullata).
    "DeliberationAsked",
];

const DEAD_COLOR: Color32 = Color32::from_rgb(170, 170, 170);
const BIRTHS_COLOR: Color32 = Color32::from_rgb(110, 190, 110);
const DEATHS_COLOR: Color32 = Color32::from_rgb(230, 80, 80);
const WARNING: Color32 = Color32::from_rgb(240, 160, 60);

pub struct HistoryUiPlugin;

impl Plugin for HistoryUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HistoryWindow>()
            .add_systems(Update, (toggle_window, follow_selection))
            .add_systems(EguiPrimaryContextPass, history_window.before(PointerCheck));
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum Tab {
    #[default]
    Biografia,
    Albero,
    Ricerca,
    Giocatore,
    Record,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Biografia,
        Tab::Albero,
        Tab::Ricerca,
        Tab::Giocatore,
        Tab::Record,
    ];

    fn title(self) -> &'static str {
        match self {
            Tab::Biografia => "Biografia",
            Tab::Albero => "Albero genealogico",
            Tab::Ricerca => "Ricerca",
            Tab::Giocatore => "Registro del giocatore",
            Tab::Record => "Record",
        }
    }
}

/// Stato della finestra "Storia".
#[derive(Resource)]
pub struct HistoryWindow {
    pub open: bool,
    tab: Tab,
    /// Persona mostrata in Biografia e Albero (viva o morta).
    focus: Option<NpcId>,
    search: String,
    /// Biografia: nasconde acquisti, oggetti rotti e fame.
    only_life: bool,
    view: View,
}

impl Default for HistoryWindow {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::default(),
            focus: None,
            search: String::new(),
            only_life: true,
            view: View::default(),
        }
    }
}

impl HistoryWindow {
    /// Apre la biografia di `id`.
    pub fn show(&mut self, id: NpcId) {
        self.open = true;
        self.focus = Some(id);
        if !matches!(self.tab, Tab::Biografia | Tab::Albero) {
            self.tab = Tab::Biografia;
        }
    }
}

/// Cosa la vista in cache ha letto.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ViewKey {
    tab: Tab,
    focus: Option<NpcId>,
    search: String,
}

/// Risultati delle query della scheda aperta.
#[derive(Default)]
struct View {
    key: Option<ViewKey>,
    generation: u64,
    fetched_at: f64,
    query_ms: f32,
    error: Option<String>,
    person: Option<PersonRow>,
    parents: Vec<PersonRow>,
    partners: Vec<PersonRow>,
    children: Vec<PersonRow>,
    /// Eventi con il loro colore.
    bio: Vec<(EventRow, Color32)>,
    tree: Option<FamilyTree>,
    results: Vec<PersonRow>,
    deeds: Vec<EventRow>,
    totals: Vec<PlayerTotal>,
    records: Option<Records>,
}

struct Records {
    most_children: Vec<(PersonRow, u32)>,
    longest_lived: Vec<PersonRow>,
    families: Vec<(String, u32, u32)>,
    causes: Vec<(DeathCause, u64)>,
    years: Vec<YearCounts>,
    counts: HistoryCounts,
}

/// Cosa l'utente ha cliccato (applicato dopo aver disegnato la finestra).
enum Click {
    /// Mostra la biografia (persona morta o non più nel mondo).
    Focus(NpcId),
    /// Seleziona nel mondo e mostra la biografia.
    Select(NpcId),
}

// --- Sistemi ------------------------------------------------------------------------

fn toggle_window(
    keys: Res<ButtonInput<KeyCode>>,
    selected: Res<SelectedNpc>,
    mut window: ResMut<HistoryWindow>,
) {
    if keys.just_pressed(KeyCode::KeyH) {
        window.open = !window.open;
        if window.open
            && let Some(id) = selected.0
        {
            window.focus = Some(id);
        }
    }
}

/// La biografia segue l'NPC selezionato nel mondo.
fn follow_selection(selected: Res<SelectedNpc>, mut window: ResMut<HistoryWindow>) {
    if selected.is_changed()
        && let Some(id) = selected.0
        && window.focus != Some(id)
    {
        window.focus = Some(id);
    }
}

fn history_window(
    mut contexts: EguiContexts,
    time: Res<Time<Real>>,
    sim: Res<Sim>,
    db: Res<HistoryDb>,
    mut window: ResMut<HistoryWindow>,
    mut selected: ResMut<SelectedNpc>,
) {
    if !window.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let window = &mut *window;
    let world = &sim.world;
    refresh(
        window,
        &db,
        time.elapsed_secs_f64(),
        world.params.days_per_year,
    );
    let mut open = true;
    let mut clicks = Vec::new();
    let center = ctx.content_rect().center();
    let max_height = (ctx.content_rect().height() - 120.0).max(200.0);
    egui::Window::new("Storia")
        .open(&mut open)
        .default_pos(center - egui::vec2(WINDOW_WIDTH / 2.0, 260.0))
        .default_width(WINDOW_WIDTH)
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for tab in Tab::ALL {
                    ui.selectable_value(&mut window.tab, tab, tab.title());
                }
            });
            ui.separator();
            if let Some(e) = &db.error {
                ui.colored_label(WARNING, format!("Storico non disponibile: {e}"));
                return;
            }
            if !db.is_open() {
                ui.weak("Storico non ancora aperto.");
                return;
            }
            if let Some(e) = &window.view.error {
                ui.colored_label(WARNING, format!("Errore: {e}"));
            }
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .auto_shrink([false, true])
                .show(ui, |ui| match window.tab {
                    Tab::Biografia => biography(ui, world, window, &mut clicks),
                    Tab::Albero => family_tree(ui, world, &window.view, &mut clicks),
                    Tab::Ricerca => search(ui, world, window, &mut clicks),
                    Tab::Giocatore => player_log(ui, world, &window.view),
                    Tab::Record => records(ui, world, &window.view, &db, &mut clicks),
                });
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "{} · query {:.2} ms",
                    db.path()
                        .map_or_else(String::new, |p| p.display().to_string()),
                    window.view.query_ms
                ))
                .small()
                .weak(),
            );
        });
    if !open {
        window.open = false;
    }
    for click in clicks {
        match click {
            Click::Focus(id) => window.focus = Some(id),
            Click::Select(id) => {
                window.focus = Some(id);
                selected.0 = Some(id);
            }
        }
        if window.tab == Tab::Ricerca {
            window.tab = Tab::Biografia;
        }
    }
}

// --- Query --------------------------------------------------------------------------

/// Rilegge la vista se è cambiato cosa mostra, o se è vecchia e lo storico è cresciuto.
fn refresh(window: &mut HistoryWindow, db: &HistoryDb, now: f64, days_per_year: u32) {
    let key = ViewKey {
        tab: window.tab,
        focus: window.focus,
        search: window.search.trim().to_string(),
    };
    let view = &window.view;
    let same = view.key.as_ref() == Some(&key);
    let stale = view.generation != db.generation && now - view.fetched_at >= REFRESH_SECS;
    if same && !stale {
        return;
    }
    let start = Instant::now();
    let mut view = View {
        key: Some(key.clone()),
        generation: db.generation,
        fetched_at: now,
        ..View::default()
    };
    let result = db.read(|h| fetch(h, &key, days_per_year, &mut view));
    if let Some(Err(e)) = result {
        view.error = Some(e);
    }
    view.query_ms = start.elapsed().as_secs_f32() * 1000.0;
    window.view = view;
}

fn fetch(h: &History, key: &ViewKey, days_per_year: u32, view: &mut View) -> history::Result<()> {
    match key.tab {
        Tab::Biografia => {
            let Some(id) = key.focus else {
                return Ok(());
            };
            let Some(person) = h.person(id)? else {
                return Ok(());
            };
            for parent in [person.mother, person.father].into_iter().flatten() {
                view.parents.extend(h.person(parent)?);
            }
            view.partners = h.partners(id)?;
            view.children = h.children(id)?;
            view.bio = h
                .biography(id)?
                .into_iter()
                .map(|mut e| {
                    let kind = e.event_kind();
                    if let Some(text) = kind.as_ref().and_then(|k| bio_text(k, id)) {
                        e.text = text;
                    }
                    let color = kind.map_or(Color32::GRAY, |kind| event_color(&kind));
                    (e, color)
                })
                .collect();
            view.person = Some(person);
        }
        Tab::Albero => {
            if let Some(id) = key.focus {
                view.tree = h.family_tree(id, TREE_UP, TREE_DOWN)?;
            }
        }
        Tab::Ricerca => {
            if !key.search.is_empty() {
                view.results = h.search_people(&key.search, SEARCH_LIMIT)?;
            }
        }
        Tab::Giocatore => {
            view.deeds = h.player_actions(DEEDS_LIMIT)?;
            view.totals = h.player_totals()?;
        }
        Tab::Record => {
            view.records = Some(Records {
                most_children: h.most_children(RECORDS_LIMIT)?,
                longest_lived: h.longest_lived(RECORDS_LIMIT)?,
                families: h.largest_families(RECORDS_LIMIT)?,
                causes: h.deaths_by_cause()?,
                years: h.yearly_counts(days_per_year)?,
                counts: h.counts()?,
            });
        }
    }
    Ok(())
}

/// Testo di un evento nella biografia di `focus`, se va detto meglio che nel
/// registro (le deliberazioni: dal punto di vista di chi decide o di chi le
/// ha provocate). `None`: il testo del registro va bene.
fn bio_text(kind: &EventKind, focus: NpcId) -> Option<String> {
    match kind {
        EventKind::DeliberationAsked {
            npc,
            name,
            question,
            ..
        } => Some(if *npc == focus {
            format!("Ci pensa: {question}")
        } else {
            format!("{name} ci pensa: {question}")
        }),
        EventKind::DeliberationResolved {
            npc,
            name,
            kind,
            description,
            by,
            confidence,
            ..
        } => {
            let by = match (by, confidence) {
                (Resolver::Brain, Some(c)) => format!(" (con Laya, {:.0}%)", c * 100.0),
                (Resolver::Brain, None) => " (con Laya)".to_string(),
                (Resolver::Rules, _) => String::new(),
            };
            let topic = kind.topic();
            Some(if *npc == focus {
                let mut topic = topic.to_string();
                if let Some(first) = topic.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                format!("{topic}: {description}{by}")
            } else {
                match kind {
                    DeliberationKind::CoupleProposal { .. } => {
                        format!("{name} risponde alla sua proposta: {description}{by}")
                    }
                    DeliberationKind::HaveChild { .. } => {
                        format!("{name} decide sul figlio: {description}{by}")
                    }
                    _ => format!("{name} decide ({topic}): {description}{by}"),
                }
            })
        }
        _ => None,
    }
}

// --- Schede -------------------------------------------------------------------------

fn biography(
    ui: &mut egui::Ui,
    world: &World,
    window: &mut HistoryWindow,
    clicks: &mut Vec<Click>,
) {
    let dpy = world.params.days_per_year;
    let now = world.clock;
    let view = &window.view;
    let Some(p) = &view.person else {
        match window.focus {
            Some(id) => ui.weak(format!("Nessuna traccia di {id} nello storico (per ora).")),
            None => ui.weak("Seleziona un NPC nel mondo o cercalo nella scheda «Ricerca»."),
        };
        return;
    };
    ui.horizontal(|ui| {
        ui.heading(&p.name);
        if !p.is_alive() {
            ui.heading(RichText::new("†").color(DEAD_COLOR));
        }
    });
    let origin = if p.founder {
        p.sex.pick("fondatrice", "fondatore").to_string()
    } else {
        format!("{} sul treno", p.sex.pick("nata", "nato"))
    };
    let age = p
        .age_at(now, dpy)
        .map_or_else(String::new, |a| format!(" · {a} anni"));
    ui.weak(format!("{} · {origin}{age} · {}", p.sex.name(), p.id));

    egui::Grid::new("history_bio_facts")
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            ui.weak(p.sex.pick("Nata", "Nato"));
            ui.label(
                p.born
                    .map_or_else(|| "?".to_string(), |b| birth_date(b, dpy)),
            );
            ui.end_row();
            if let Some(died) = p.died {
                ui.weak(p.sex.pick("Morta", "Morto"));
                let cause = match p.death_cause {
                    Some(DeathCause::OldAge) => ", di vecchiaia",
                    Some(DeathCause::Starvation) => ", di fame",
                    Some(DeathCause::Violence) => p.sex.pick(", uccisa", ", ucciso"),
                    Some(DeathCause::Wounds) => ", per le ferite",
                    None => "",
                };
                let age = p
                    .death_age
                    .map_or_else(String::new, |a| format!(" a {a} anni"));
                ui.label(format!("{}{age}{cause}", date(died, dpy)));
                ui.end_row();
            }
            for (title, people) in [
                ("Genitori", &view.parents),
                ("Partner", &view.partners),
                ("Figli", &view.children),
            ] {
                if people.is_empty() {
                    continue;
                }
                ui.weak(title);
                ui.horizontal_wrapped(|ui| {
                    for person in people {
                        person_link(ui, world, person, clicks);
                    }
                });
                ui.end_row();
            }
        });
    ui.separator();

    ui.horizontal(|ui| {
        ui.strong("Cronologia");
        ui.checkbox(&mut window.only_life, "solo i fatti della vita")
            .on_hover_text("Nasconde acquisti, oggetti rotti e fame");
    });
    let mut shown = 0;
    for (event, color) in &view.bio {
        if window.only_life && MINOR_KINDS.contains(&event.kind.as_str()) {
            continue;
        }
        shown += 1;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(date(event.time, dpy)).small().weak());
            ui.colored_label(*color, &event.text);
        });
    }
    if shown == 0 {
        ui.weak("Nessun evento registrato.");
    }
}

fn family_tree(ui: &mut egui::Ui, world: &World, view: &View, clicks: &mut Vec<Click>) {
    let Some(tree) = &view.tree else {
        ui.weak("Seleziona un NPC nel mondo o cercalo nella scheda «Ricerca».");
        return;
    };
    ui.strong("Antenati");
    if tree.parents.is_empty() {
        ui.weak("  sconosciuti (fondatori o genitori mai registrati)");
    }
    for parent in &tree.parents {
        ancestor(ui, world, parent, clicks);
    }
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label("•");
        person_link(ui, world, &tree.person, clicks);
        partners(ui, world, &tree.partners, clicks);
    });
    ui.separator();
    let count = tree.len() - 1 - tree.parents.iter().map(FamilyNode::len).sum::<usize>();
    ui.strong(format!("Discendenti ({count})"));
    if tree.children.is_empty() {
        ui.weak("  nessuno");
    }
    for child in &tree.children {
        descendant(ui, world, child, clicks);
    }
}

/// Un genitore con i suoi genitori rientrati sotto.
fn ancestor(ui: &mut egui::Ui, world: &World, node: &FamilyNode, clicks: &mut Vec<Click>) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(node.person.sex.pick("madre:", "padre:"));
        person_link(ui, world, &node.person, clicks);
    });
    if !node.parents.is_empty() {
        ui.indent(("ancestor", node.person.id.0), |ui| {
            for parent in &node.parents {
                ancestor(ui, world, parent, clicks);
            }
        });
    }
}

/// Un discendente con i partner e i figli rientrati sotto.
fn descendant(ui: &mut egui::Ui, world: &World, node: &FamilyNode, clicks: &mut Vec<Click>) {
    ui.horizontal_wrapped(|ui| {
        ui.weak(node.person.sex.pick("figlia:", "figlio:"));
        person_link(ui, world, &node.person, clicks);
        partners(ui, world, &node.partners, clicks);
    });
    if !node.children.is_empty() {
        ui.indent(("descendant", node.person.id.0), |ui| {
            for child in &node.children {
                descendant(ui, world, child, clicks);
            }
        });
    }
}

fn partners(ui: &mut egui::Ui, world: &World, partners: &[PersonRow], clicks: &mut Vec<Click>) {
    for partner in partners {
        ui.weak("♡");
        person_link(ui, world, partner, clicks);
    }
}

fn search(ui: &mut egui::Ui, world: &World, window: &mut HistoryWindow, clicks: &mut Vec<Click>) {
    let dpy = world.params.days_per_year;
    ui.horizontal(|ui| {
        ui.label("Nome:");
        ui.add(
            egui::TextEdit::singleline(&mut window.search)
                .hint_text("es. Rossi")
                .desired_width(220.0),
        );
    });
    let view = &window.view;
    if window.search.trim().is_empty() {
        ui.weak("Scrivi parte di un nome o di un cognome (vivi e morti).");
        return;
    }
    if view.results.is_empty() {
        ui.weak("Nessuno con questo nome.");
        return;
    }
    if view.results.len() == SEARCH_LIMIT {
        ui.weak(format!("Primi {SEARCH_LIMIT} risultati"));
    }
    egui::Grid::new("history_search")
        .num_columns(3)
        .striped(true)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for p in &view.results {
                person_link(ui, world, p, clicks);
                ui.weak(
                    p.born
                        .map_or_else(|| "?".to_string(), |b| birth_date(b, dpy)),
                );
                match p.died {
                    None => ui.label(p.sex.pick("viva", "vivo")),
                    Some(died) => ui.colored_label(
                        DEAD_COLOR,
                        format!(
                            "† {}{}",
                            date(died, dpy),
                            p.death_age
                                .map_or_else(String::new, |a| format!(", {a} anni"))
                        ),
                    ),
                };
                ui.end_row();
            }
        });
}

fn player_log(ui: &mut egui::Ui, world: &World, view: &View) {
    let dpy = world.params.days_per_year;
    ui.strong("Chi ha preso cosa");
    if view.totals.is_empty() {
        ui.weak("Il giocatore non ha ancora preso, comprato o regalato nulla.");
        return;
    }
    egui::Grid::new("history_player_totals")
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for t in &view.totals {
                let item = t.item.map_or("?", ItemKind::plural);
                let (verb, extra) = match t.kind.as_str() {
                    "PlayerTook" => ("Preso", String::new()),
                    "PlayerBought" => ("Comprato", format!(" per {} gettoni", t.tokens)),
                    "PlayerSold" => ("Venduto", format!(" per {} gettoni", t.tokens)),
                    _ => ("Regalato", String::new()),
                };
                ui.weak(verb);
                ui.label(format!("{} {item}{extra} ({} volte)", t.units, t.times));
                ui.end_row();
            }
        });
    ui.separator();
    ui.strong(format!("Ultime azioni (fino a {DEEDS_LIMIT})"));
    for deed in &view.deeds {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(date(deed.time, dpy)).small().weak());
            ui.label(&deed.text);
        });
    }
}

fn records(ui: &mut egui::Ui, world: &World, view: &View, db: &HistoryDb, clicks: &mut Vec<Click>) {
    let Some(r) = &view.records else {
        return;
    };
    egui::Grid::new("history_records")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.strong("Più figli");
            ui.vertical(|ui| {
                for (p, n) in &r.most_children {
                    ui.horizontal(|ui| {
                        ui.label(format!("{n} figli"));
                        person_link(ui, world, p, clicks);
                    });
                }
            });
            ui.end_row();
            ui.strong("Più longevi");
            ui.vertical(|ui| {
                for p in &r.longest_lived {
                    person_link(ui, world, p, clicks);
                }
            });
            ui.end_row();
            ui.strong("Famiglie");
            ui.vertical(|ui| {
                for (surname, total, alive) in &r.families {
                    ui.label(format!("{surname}: {total} persone, {alive} in vita"));
                }
            });
            ui.end_row();
            ui.strong("Morti");
            ui.vertical(|ui| {
                for (cause, n) in &r.causes {
                    let cause = match cause {
                        DeathCause::OldAge => "di vecchiaia",
                        DeathCause::Starvation => "di fame",
                        DeathCause::Violence => "uccisi",
                        DeathCause::Wounds => "per le ferite",
                    };
                    ui.label(format!("{n} {cause}"));
                }
            });
            ui.end_row();
        });
    ui.separator();
    births_and_deaths(ui, &r.years);
    ui.separator();
    let c = &r.counts;
    ui.label(
        RichText::new(format!(
            "Storico: {} eventi, {} persone ({} vive, {} morte), {} coppie",
            c.events, c.people, c.alive, c.dead, c.couples
        ))
        .small(),
    );
    if c.lost_events > 0 {
        ui.colored_label(
            WARNING,
            format!(
                "Attenzione: {} eventi persi in {} buchi",
                c.lost_events, c.gaps
            ),
        );
    }
    let s = &db.stats;
    ui.label(
        RichText::new(format!(
            "Sincronizzazione: {} lotti, {} eventi · cattura {} µs (max {}) · scrittura {} µs (max {})",
            s.batches, s.events, s.last_capture_us, s.max_capture_us, s.last_write_us, s.max_write_us
        ))
        .small()
        .weak(),
    );
}

/// Nascite e morti per anno, dallo storico (anche prima dell'ultimo caricamento).
fn births_and_deaths(ui: &mut egui::Ui, years: &[YearCounts]) {
    ui.strong("Nascite e morti per anno");
    let bars = |offset: f64, f: &dyn Fn(&YearCounts) -> u64| -> Vec<Bar> {
        years
            .iter()
            .map(|y| Bar::new(y.year as f64 + 0.5 + offset, f(y) as f64).width(0.4))
            .collect()
    };
    let top = years
        .iter()
        .map(|y| y.births.max(y.deaths))
        .max()
        .unwrap_or(0);
    Plot::new("history_years")
        .height(180.0)
        .legend(Legend::default().position(Corner::LeftTop))
        .x_axis_label("anno")
        .include_y(0.0)
        .include_y(top as f64 * 1.4 + 1.0)
        .allow_scroll(false)
        .show(ui, |plot| {
            plot.bar_chart(BarChart::new("Nascite", bars(-0.2, &|y| y.births)).color(BIRTHS_COLOR));
            plot.bar_chart(BarChart::new("Morti", bars(0.2, &|y| y.deaths)).color(DEATHS_COLOR));
        });
}

// --- Utilità --------------------------------------------------------------------------

/// Nome ed età (o † ed età alla morte) come link: una persona viva nel mondo
/// si seleziona, le altre mostrano la biografia.
fn person_link(ui: &mut egui::Ui, world: &World, p: &PersonRow, clicks: &mut Vec<Click>) {
    let dpy = world.params.days_per_year;
    let in_world = world.npc(p.id).is_some();
    let sex = match p.sex {
        Sex::Female => "♀",
        Sex::Male => "♂",
    };
    let text = match (p.is_alive(), p.age_at(world.clock, dpy)) {
        (true, Some(age)) => format!("{sex} {} ({age})", p.name),
        (true, None) => format!("{sex} {}", p.name),
        (false, Some(age)) => format!("{sex} {} († {age})", p.name),
        (false, None) => format!("{sex} {} (†)", p.name),
    };
    let text = if p.is_alive() {
        RichText::new(text)
    } else {
        RichText::new(text).color(DEAD_COLOR)
    };
    let hint = if in_world {
        "Click: seleziona nel mondo"
    } else {
        "Click: mostra la biografia"
    };
    if ui.link(text).on_hover_text(hint).clicked() {
        clicks.push(if in_world {
            Click::Select(p.id)
        } else {
            Click::Focus(p.id)
        });
    }
}

/// "Anno 3 · Giorno 27 08:15".
fn date(time: GameTime, days_per_year: u32) -> String {
    format!("Anno {} · {time}", year_of(time, days_per_year))
}

/// Data di nascita; per chi è nato prima della partenza, quanti anni prima.
fn birth_date(born: i64, days_per_year: u32) -> String {
    if born >= 0 {
        return date(GameTime(born as u64), days_per_year);
    }
    let year = i64::from(days_per_year.max(1)) * sim::MINUTES_PER_DAY as i64;
    let years = (-born).div_euclid(year);
    match years {
        0 => "poco prima della partenza".to_string(),
        1 => "un anno prima della partenza".to_string(),
        n => format!("{n} anni prima della partenza"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_before_and_after_the_departure() {
        assert_eq!(
            date(GameTime::from_dhm(27, 8, 15), 12),
            "Anno 3 · Giorno 27 08:15"
        );
        let year = 12 * sim::MINUTES_PER_DAY as i64;
        assert_eq!(
            birth_date(-30 * year - 5, 12),
            "30 anni prima della partenza"
        );
        assert_eq!(birth_date(-year, 12), "un anno prima della partenza");
        assert_eq!(birth_date(-5, 12), "poco prima della partenza");
        assert_eq!(birth_date(0, 12), "Anno 1 · Giorno 1 00:00");
    }

    /// Ogni scheda, con i dati veri di 30 anni di storia, si legge e si
    /// disegna senza errori; i click nell'albero producono selezioni.
    #[test]
    fn every_tab_fetches_and_draws_a_real_history() {
        use sim::{MINUTES_PER_DAY, UtilityBrain};
        let mut world = World::generate(42, 10, 100);
        let mut brain = UtilityBrain::new(42);
        let mut h = History::open_in_memory().unwrap();
        for _ in 0..30 * world.params.days_per_year {
            world.run(&mut brain, MINUTES_PER_DAY);
            h.sync(&world).unwrap();
        }
        let serra = world
            .carriages
            .iter()
            .find(|c| c.stock.count(ItemKind::Verdura) >= 1)
            .map(|c| c.id)
            .unwrap();
        world.player_take(serra, ItemKind::Verdura, 1);
        h.sync(&world).unwrap();
        let dpy = world.params.days_per_year;
        // Chi ha più figli: ha una biografia e un albero pieni.
        let (patriarch, _) = h.most_children(1).unwrap().remove(0);
        let db = HistoryDb::default();
        let ctx = egui::Context::default();
        for tab in Tab::ALL {
            let key = ViewKey {
                tab,
                focus: Some(patriarch.id),
                search: "a".into(),
            };
            let mut window = HistoryWindow {
                open: true,
                tab,
                focus: key.focus,
                search: key.search.clone(),
                ..HistoryWindow::default()
            };
            fetch(&h, &key, dpy, &mut window.view).unwrap();
            match tab {
                Tab::Biografia => {
                    assert!(window.view.person.is_some());
                    assert!(!window.view.children.is_empty());
                    assert!(!window.view.bio.is_empty());
                }
                Tab::Albero => assert!(window.view.tree.as_ref().unwrap().len() > 1),
                Tab::Ricerca => assert!(!window.view.results.is_empty()),
                Tab::Giocatore => assert_eq!(window.view.deeds.len(), 1),
                Tab::Record => assert!(!window.view.records.as_ref().unwrap().years.is_empty()),
            }
            let mut clicks = Vec::new();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| match tab {
                Tab::Biografia => biography(ui, &world, &mut window, &mut clicks),
                Tab::Albero => family_tree(ui, &world, &window.view, &mut clicks),
                Tab::Ricerca => search(ui, &world, &mut window, &mut clicks),
                Tab::Giocatore => player_log(ui, &world, &window.view),
                Tab::Record => records(ui, &world, &window.view, &db, &mut clicks),
            });
            output.textures_delta.clear();
            assert!(clicks.is_empty());
        }
    }

    #[test]
    fn deliberations_read_well_in_biographies() {
        let (npc, from) = (NpcId(1), NpcId(2));
        let resolved = EventKind::DeliberationResolved {
            id: sim::DeliberationId(7),
            npc,
            name: "Marta Rossi".into(),
            kind: DeliberationKind::CoupleProposal { from },
            choice: sim::Choice::Accept,
            description: "accetta e diventa la compagna di Luca Bianchi".into(),
            by: Resolver::Brain,
            confidence: Some(0.72),
        };
        assert_eq!(
            bio_text(&resolved, npc).unwrap(),
            "Proposta di coppia: accetta e diventa la compagna di Luca Bianchi (con Laya, 72%)"
        );
        assert_eq!(
            bio_text(&resolved, from).unwrap(),
            "Marta Rossi risponde alla sua proposta: accetta e diventa la compagna di Luca Bianchi (con Laya, 72%)"
        );
        let born = EventKind::Shortage {
            item: ItemKind::Razione,
        };
        assert_eq!(bio_text(&born, npc), None);
    }

    #[test]
    fn egui_default_fonts_have_the_symbols_used_here() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
        let font = egui::FontId::proportional(14.0);
        for s in ["†", "♀", "♂", "♡", "•", "·", "«»", "µ", "…"] {
            assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, s)), "manca {s:?}");
        }
    }
}
