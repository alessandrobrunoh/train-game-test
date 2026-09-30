//! Screenshot automatici, per controllare la grafica senza giocare:
//! `TRAINGAME_SHOTS=<cartella> cargo run -p game`.
//!
//! Porta la simulazione alla sera (chi dorme, dorme anche al piano di sopra),
//! porta il giocatore davanti alla prima carrozza a più piani, salva uno
//! screenshot per scena (piano terra, piano di sopra, sulla scala, vista
//! allargata, la cabina del giocatore con un amico che lo saluta, la chat
//! con l'amico) nella cartella e chiude il gioco. Senza la variabile non fa
//! nulla.
//!
//! La camera disegna su un'immagine fuori schermo, non sulla finestra: macOS
//! non disegna le finestre coperte o con lo schermo bloccato (screenshot
//! neri). Anche le finestre egui finiscono nell'immagine: l'ultima scena ha
//! inventario e baule aperti.
//!
//! Le scene 8–10 mostrano il Narratore: la cronaca (N) con i pannelli
//! aperti, le statistiche (K) e tutte le icone procedurali. L'ultima è una
//! rissa: barre della salute, numeri del danno, grida e l'ispettore sulla
//! vittima (vedi `combat.rs`). Poi le bande: la finestra (J) con due bande,
//! le fasce sul braccio dei membri e l'ispettore su uno di loro; infine la
//! chat con un amico che invita il giocatore nella sua banda. La cronaca è
//! finta e fissa ([`FIXTURES`], alcune sono risposte vere di un modello),
//! quindi gli screenshot sono uguali a ogni giro e non usano la rete. Con
//! `TRAINGAME_SHOTS_LLM=1` invece il Narratore è quello di `.env`: chiede la
//! novità del giorno 1 alle 6:00 e la cronaca mostra la risposta vera.

use std::path::PathBuf;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use sim::GameTime;

use crate::ai_ui::StatsWindow;
use crate::cabin::ChestWindow;
use crate::camera::WideView;
use crate::chat::{ChatCommand, ChatQueue};
use crate::chronicle_ui::ChronicleWindow;
use crate::inventory::InventoryWindow;
use crate::item_icons::{ItemIcons, icon_canvas, shape_canvas, show_icon};
use crate::narrator_bridge::{ChronicleEntry, EntryStatus, NarratorState};
use crate::player::{Body, Player, start_position};
use crate::state::{SelectedNpc, Sim};
use crate::stations::StationLayout;
use crate::train::{FLOOR_Y, STAIRS_WIDTH, TrainLayout, floor_y};

const SHOTS_ENV: &str = "TRAINGAME_SHOTS";
/// Con questa variabile le scene del Narratore usano il modello di `.env`.
const REAL_LLM_ENV: &str = "TRAINGAME_SHOTS_LLM";
/// Attesa prima della prima scena (arte generata, camera ferma)...
const FIRST_WAIT: f32 = 4.0;
/// ...e tra una scena e l'altra (la camera raggiunge il giocatore).
const SCENE_WAIT: f32 = 2.5;
/// Ora del giorno 1 a cui si porta la simulazione prima delle scene.
const EVENING: (u64, u64) = (22, 30);

pub struct ShotPlugin;

impl Plugin for ShotPlugin {
    fn build(&self, app: &mut App) {
        let Some(dir) = std::env::var_os(SHOTS_ENV).filter(|d| !d.is_empty()) else {
            return;
        };
        let real = std::env::var_os(REAL_LLM_ENV).is_some_and(|v| !v.is_empty() && v != "0");
        if !real {
            // Niente rete: un Narratore finto, fermo, con la cronaca di prova.
            let llm = std::sync::Arc::new(llm::MockLlm::fixed(FIXTURES[0].1));
            let mut state = NarratorState::new(
                narrator::NarratorConfig::new(llm, llm::Budget::per_hour(0)),
                "finto (screenshot)",
            );
            state.paused = true;
            app.insert_resource(state);
        }
        app.insert_resource(ShotScript {
            dir: PathBuf::from(dir),
            scene: 0,
            wait: FIRST_WAIT,
            target: Handle::default(),
            real,
            captured: false,
            gang_friend: None,
        })
        .init_resource::<IconGallery>()
        .add_systems(PostStartup, render_offscreen)
        .add_systems(Update, run_script)
        .add_systems(
            bevy_egui::EguiPrimaryContextPass,
            icon_gallery.before(crate::ui::PointerCheck),
        );
    }
}

#[derive(Resource)]
struct ShotScript {
    dir: PathBuf,
    /// Prossima scena da preparare (`SCENES.len()` = fine).
    scene: usize,
    /// Secondi reali prima di scattare la scena preparata.
    wait: f32,
    /// Immagine su cui disegna la camera.
    target: Handle<Image>,
    /// Il Narratore è quello vero (`TRAINGAME_SHOTS_LLM`).
    real: bool,
    /// La scena preparata è già stata scattata: si prepara la prossima al
    /// giro dopo, così le finestre della scena restano nella foto.
    captured: bool,
    /// L'amico del giocatore che lo invita nella sua banda (scene 13–14).
    gang_friend: Option<sim::NpcId>,
}

/// La finestra con tutte le icone (solo per gli screenshot).
#[derive(Resource, Default)]
struct IconGallery(bool);

/// La cronaca di prova: giorno e risposta del modello. Le voci 2, 3 e 5 sono
/// risposte vere di un modello (misura dell'A3 con i pannelli).
const FIXTURES: [(u64, &str); 6] = [
    (
        1,
        r#"{"motivo": "In coda si beve poco e male: l'acqua scende a secchi dalla Mensa.", "novita": {"tipo": "oggetto", "nome": "Borraccia di latta", "descrizione": "Una borraccia battuta a mano da un rottame: tiene l'acqua calda per ore.", "categoria": "durevole", "valore": 14, "pila": 3, "ingredienti": [{"oggetto": "metallo", "qta": 1}], "lavoro": "operaio", "aspetto": {"forma": "bottiglia", "colore": "grigio", "dettaglio": "etichetta"}}, "interfaccia": {"titolo": "Acqua in coda", "elementi": [{"tipo": "valore", "etichetta": "Borracce sul treno", "sorgente": {"scorta": "Borraccia di latta"}, "formato": "numero"}, {"tipo": "valore", "etichetta": "Metallo al Mercato", "sorgente": {"prezzo": "metallo", "mercato": "vicino"}, "formato": "gettoni"}, {"tipo": "barra", "etichetta": "Sazietà nei Dormitori", "sorgente": {"bisogno": "sazieta", "carrozza": "Dormitorio"}, "min": 0, "max": 1}, {"tipo": "lista", "etichetta": "Dove c'è rottame", "sorgente_lista": {"carrozze_con": "rottame"}}, {"tipo": "pulsante", "etichetta": "Batti una borraccia", "azione": {"apri_crafting": "Borraccia di latta"}}, {"tipo": "pulsante", "etichetta": "Chiedi a un operaio", "azione": {"parla_con_lavoro": "operaio"}}]}}"#,
    ),
    (
        2,
        r#"{"motivo": "Le scorte di rottame sono quasi esaurite, impedendo la manutenzione e la produzione di lampade.", "novita": {"tipo": "oggetto", "nome": "Frammento Recuperato", "descrizione": "Piccoli pezzi di metallo ossidato recuperati dalle intercapedini delle carrozze.", "categoria": "materia_prima", "valore": 5, "pila": 20, "ingredienti": [], "lavoro": "operaio", "aspetto": {"forma": "lingotto", "colore": "grigio", "dettaglio": "crepa"}}, "interfaccia": {"titolo": "Recupero Rottami", "elementi": [{"tipo": "valore", "etichetta": "Rottame attuale", "sorgente": {"scorta": "rottame"}, "formato": "numero"}, {"tipo": "pulsante", "etichetta": "Raccogli frammenti", "azione": {"apri_crafting": "Frammento Recuperato"}}]}}"#,
    ),
    (
        3,
        r#"{"motivo": "La disuguaglianza economica è presente e i gettoni mediani sono bassi; serve un modo per monitorare se la ricchezza è concentrata in poche mani.", "novita": {"tipo": "statistica", "nome": "Indice di Privilegio", "descrizione": "Misura il divario tra l'élite e la massa del treno.", "unita": "punti", "scala": [0, 200], "formula": {"somma": [{"peso": 1, "sorgente": {"economia": "disuguaglianza"}}, {"peso": 100, "sorgente": {"economia": "gettoni_mediani"}}]}, "soglie": [{"sopra": 150, "testo": "Tensione sociale esplosiva"}, {"sotto": 50, "testo": "Distribuzione equa"}]}, "interfaccia": {"titolo": "Stato Sociale", "elementi": [{"tipo": "valore", "etichetta": "Indice di Privilegio", "sorgente": {"statistica": "Indice di Privilegio"}, "formato": "numero"}, {"tipo": "valore", "etichetta": "Disuguaglianza", "sorgente": {"economia": "disuguaglianza"}, "formato": "percento"}, {"tipo": "valore", "etichetta": "Gettoni Medi", "sorgente": {"economia": "gettoni_mediani"}, "formato": "gettoni"}]}}"#,
    ),
    (
        4,
        r#"{"motivo": "Nessun numero dice quanto regge l'animo del treno dopo giorni di gelo.", "novita": {"tipo": "statistica", "nome": "Morale", "descrizione": "Quanto regge l'animo di chi vive sul treno.", "unita": "%", "scala": [0, 100], "formula": {"media": [{"peso": 100, "sorgente": {"bisogno": "sazieta"}}, {"peso": 100, "sorgente": {"bisogno": "socialita"}}, {"peso": 100, "sorgente": {"bisogno": "energia"}}]}, "soglie": [{"sotto": 30, "testo": "Il treno è allo stremo"}, {"sopra": 75, "testo": "Si canta nelle carrozze"}]}, "interfaccia": {"titolo": "Morale del treno", "elementi": [{"tipo": "valore", "etichetta": "Morale", "sorgente": {"statistica": "Morale"}, "formato": "numero"}, {"tipo": "barra", "etichetta": "Energia", "sorgente": {"bisogno": "energia"}, "min": 0, "max": 100}, {"tipo": "lista", "etichetta": "I più affamati", "sorgente_lista": {"affamati": "treno"}}, {"tipo": "pulsante", "etichetta": "Storico", "azione": {"mostra_statistica": "Morale"}}]}}"#,
    ),
    (
        5,
        r#"{"motivo": "La popolazione è stabile e le scorte di cibo sono sufficienti, ma la mancanza di interazione tra le diverse carrozze sta appiattendo il morale.", "novita": {"tipo": "evento", "titolo": "Il Banchetto della Luna", "descrizione": "I cuochi hanno organizzato una cena celebrativa per unire i passeggeri, consumando scorte extra per aumentare l'umore generale.", "effetti": [{"effetto": "scorta", "carrozza": "Mensa", "oggetto": "razione", "delta": -30}, {"effetto": "scorta", "carrozza": "Mensa", "oggetto": "tè", "delta": -20}, {"effetto": "bisogno", "carrozza": null, "bisogno": "socialita", "delta": 0.2}]}, "interfaccia": {"titolo": "Festa nel Treno", "elementi": [{"tipo": "testo", "testo": "Un momento di gioia collettiva tra i vagoni."}, {"tipo": "valore", "etichetta": "Razioni consumate", "sorgente": {"scorta": "razione"}, "formato": "numero"}, {"tipo": "barra", "etichetta": "Socialità", "sorgente": {"bisogno": "socialita"}, "min": 0, "max": 1}]}}"#,
    ),
    (
        6,
        r#"{"motivo": "Le coperte non bastano e i Dormitori di coda gelano.", "novita": {"tipo": "lavoro", "nome": "Rammendatore", "descrizione": "Ripara vestiti e coperte logore con i ritagli dell'Officina.", "carrozza": "Officina", "produce": ["vestito", "coperta"]}, "interfaccia": {"titolo": "Rammendi", "elementi": [{"tipo": "valore", "etichetta": "Coperte sul treno", "sorgente": {"scorta": "coperta"}}, {"tipo": "pulsante", "etichetta": "Cuci una coperta", "azione": {"apri_crafting": "coperta"}}, {"tipo": "pulsante", "etichetta": "Prezzo dei vestiti", "azione": {"apri_mercato": "vestito"}}, {"tipo": "pulsante", "etichetta": "Dov'è l'Officina", "azione": {"vai_a": "Officina"}}]}}"#,
    ),
];

/// Mette la cronaca di prova nel Narratore (e un rifiuto, per vederlo): le
/// bozze accettate vanno al Custode di `world`, che le esamina all'ora piena.
fn stage_chronicle(state: &mut NarratorState, world: &mut sim::World, now: f64) {
    for (day, answer) in FIXTURES {
        let draft = narrator::parse(answer).expect("a valid fixture");
        state.record(
            ChronicleEntry {
                day,
                status: EntryStatus::Proposed,
                draft: Some(draft),
                reason: None,
                attempts: 1,
                latency_ms: 1300,
                seq: None,
                at: None,
                summary: None,
            },
            now,
            Some(world),
        );
        if day == 3 {
            state.record(
                ChronicleEntry {
                    day,
                    status: EntryStatus::Rejected,
                    draft: narrator::parse(r#"{"motivo": "Il treno ha bisogno di acqua pulita.", "novita": {"tipo": "lavoro", "nome": "Acquaiolo", "descrizione": "Porta l'acqua dalle Mense ai Dormitori.", "carrozza": "Cisterna", "produce": ["acqua"]}}"#).ok(),
                    reason: Some("la carrozza «Cisterna» non esiste; tipi di carrozza: Dormitorio, Mensa, Mercato, Officina, Serra".into()),
                    attempts: 2,
                    latency_ms: 2600,
                    seq: None,
                    at: None,
                    summary: None,
                },
                now,
                None,
            );
        }
    }
}

/// Dimensioni dell'immagine fuori schermo (16:9, come la finestra).
const SHOT_SIZE: (u32, u32) = (1920, 1080);

/// Fa disegnare la camera su un'immagine invece che sulla finestra.
fn render_offscreen(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut script: ResMut<ShotScript>,
    camera: Single<Entity, With<Camera2d>>,
) {
    let image = Image::new_target_texture(
        SHOT_SIZE.0,
        SHOT_SIZE.1,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    script.target = images.add(image);
    commands
        .entity(*camera)
        .insert(RenderTarget::Image(script.target.clone().into()));
}

/// Dove mettere il giocatore in una scena.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    /// Nella prima carrozza a più piani: piano e x locale (`None` = sulla scala).
    Floor(usize, Option<f32>),
    /// Nella sua cabina, accanto al letto; con inventario e baule aperti.
    Cabin { windows: bool },
    /// Nella sua cabina, in chat con l'amico.
    Chat,
    /// Le finestre del Narratore: cronaca (N), statistiche (K), icone.
    Narrator(NarratorView),
    /// Nel primo Mercato, con la scheda "Banchi" aperta.
    Market,
    /// Una rissa nella prima carrozza a più piani, con l'ispettore aperto.
    Fight,
    /// Due bande attorno al giocatore: la finestra delle bande (J) e
    /// l'ispettore su un membro; con `invite` la chat con l'amico che invita.
    Gangs { invite: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum NarratorView {
    Chronicle,
    Stats,
    Icons,
}

/// Nome del file, posto del giocatore, vista allargata.
const SCENES: [(&str, Spot, bool); 14] = [
    ("1-piano-terra", Spot::Floor(0, Some(160.0)), false),
    ("2-piano-sopra", Spot::Floor(1, Some(160.0)), false),
    ("3-sulla-scala", Spot::Floor(0, None), false),
    ("4-vista-allargata", Spot::Floor(1, Some(120.0)), true),
    ("5-cabina", Spot::Cabin { windows: false }, false),
    ("6-inventario-baule", Spot::Cabin { windows: true }, false),
    ("7-chat", Spot::Chat, false),
    ("8-cronaca", Spot::Narrator(NarratorView::Chronicle), false),
    ("9-statistiche", Spot::Narrator(NarratorView::Stats), false),
    ("10-icone", Spot::Narrator(NarratorView::Icons), false),
    ("11-banchi", Spot::Market, false),
    ("12-rissa", Spot::Fight, false),
    ("13-bande", Spot::Gangs { invite: false }, false),
    ("14-invito", Spot::Gangs { invite: true }, false),
];

/// Una rissa attorno al giocatore nella carrozza `index`, al piano terra:
/// uno lo picchia, due si picchiano tra loro (la vittima già ferita), e
/// una quarta persona, ferita, guarda. Restituisce la vittima, da mostrare
/// nell'ispettore.
fn stage_fight(world: &mut sim::World, index: usize) -> Option<sim::NpcId> {
    let place = sim::Place {
        carriage: sim::CarriageId(index as u16),
        floor: 0,
    };
    world.set_player_place(place);
    world.player.health = 72.0;
    let people: Vec<usize> = (0..world.npcs.len())
        .filter(|&i| (20..=55).contains(&world.npcs[i].age))
        .take(4)
        .collect();
    let now = world.clock;
    for &i in &people {
        let npc = &mut world.npcs[i];
        if let Some(s) = npc.action.station() {
            let station = &mut world.carriages[npc.carriage.index()].stations[s.index()];
            station.occupancy = station.occupancy.saturating_sub(1);
        }
        npc.carriage = place.carriage;
        npc.floor = 0;
        npc.action = sim::Action::Idle;
        npc.action_since = now;
        npc.action_until = now + 60;
    }
    let &[a, b, c, d] = people.as_slice() else {
        return None;
    };
    let ids = [a, b, c, d].map(|i| world.npcs[i].id);
    world.npcs[c].health = 45.0;
    world.npcs[c].injury = 30.0;
    world.npcs[d].health = 20.0;
    world.npcs[d].injury = 50.0;
    world.npc_attack(ids[0], sim::Fighter::Player, sim::Motive::Grudge);
    world.npc_attack(ids[1], sim::Fighter::Npc(ids[2]), sim::Motive::Quarrel);
    Some(ids[2])
}

/// Due bande attorno al giocatore nella carrozza `index`, al piano terra:
/// tre membri di una e due dell'altra, fermi lì; il secondo membro della
/// prima è amico del giocatore (lo inviterà). Restituisce il primo membro,
/// da mostrare nell'ispettore, e l'amico.
fn stage_gangs(world: &mut sim::World, index: usize) -> Option<(sim::NpcId, sim::NpcId)> {
    let place = sim::Place {
        carriage: sim::CarriageId(index as u16),
        floor: 0,
    };
    world.set_player_place(place);
    world.player.health = world.player.health.max(80.0);
    // Chi non è già nella scena della rissa.
    let people: Vec<usize> = (0..world.npcs.len())
        .filter(|&i| (20..=55).contains(&world.npcs[i].age))
        .skip(4)
        .take(5)
        .collect();
    if people.len() < 5 {
        return None;
    }
    let now = world.clock;
    for &i in &people {
        let npc = &mut world.npcs[i];
        if let Some(s) = npc.action.station() {
            let station = &mut world.carriages[npc.carriage.index()].stations[s.index()];
            station.occupancy = station.occupancy.saturating_sub(1);
        }
        let npc = &mut world.npcs[i];
        npc.carriage = place.carriage;
        npc.floor = 0;
        npc.action = sim::Action::Idle;
        npc.action_since = now;
        npc.action_until = now + 120;
        npc.health = npc.health.max(80.0);
    }
    let ids: Vec<sim::NpcId> = people.iter().map(|&i| world.npcs[i].id).collect();
    world.found_gang(&ids[..3])?;
    world.found_gang(&ids[3..])?;
    world.npcs[people[1]].player = Some(sim::PlayerTie {
        affinity: 0.9,
        ..sim::PlayerTie::default()
    });
    Some((ids[0], ids[1]))
}

/// Un amico del giocatore sveglio nella cabina, e qualcosa nell'inventario
/// e nel baule: la scena mostra il saluto e le finestre piene. Restituisce l'amico.
fn stage_cabin(world: &mut sim::World) -> Option<sim::NpcId> {
    let home = world.player.home?;
    world.set_player_place(home.place());
    if world.player.inventory.is_empty() {
        for (item, n) in [
            (sim::ItemKind::Rottame, 14),
            (sim::ItemKind::Verdura, 3),
            (sim::ItemKind::Te, 2),
            (sim::ItemKind::Attrezzo, 1),
        ] {
            world.player.inventory.add(item, n);
        }
        world.player.chest.add(sim::ItemKind::Tessuto, 7);
        world.player.chest.add(sim::ItemKind::Coperta, 2);
        // Gli oggetti che il Custode ha fatto entrare nel mondo, con la loro icona.
        let added: Vec<sim::ItemKind> = world
            .catalog()
            .kinds()
            .filter(|k| !k.is_builtin())
            .collect();
        for item in added {
            world.player.inventory.add(item, 2);
        }
    }
    let i = world
        .npcs
        .iter()
        .position(|n| n.carriage == home.carriage && n.age >= 18)?;
    let now = world.clock;
    let npc = &mut world.npcs[i];
    if let Some(s) = npc.action.station() {
        let station = &mut world.carriages[npc.carriage.index()].stations[s.index()];
        station.occupancy = station.occupancy.saturating_sub(1);
    }
    npc.floor = home.floor;
    npc.action = sim::Action::Idle;
    npc.action_since = now;
    npc.action_until = now + 60;
    if npc.player.is_none() {
        npc.player = Some(sim::PlayerTie {
            affinity: 0.8,
            ..sim::PlayerTie::default()
        });
    }
    Some(npc.id)
}

/// Il giocatore nel Mercato `market` con un banco aperto (un oggetto del
/// Custode, se c'è) e un NPC che vende lì: la scheda "Banchi" piena.
fn stage_stalls(world: &mut sim::World, market: sim::CarriageId) {
    world.set_player_place(sim::Place {
        carriage: market,
        floor: 0,
    });
    let added = world.catalog().kinds().find(|k| !k.is_builtin());
    let item = added.unwrap_or(sim::ItemKind::Rottame);
    if world.player.inventory.count(item) == 0 {
        world.player.inventory.add(item, 3);
    }
    let quote = world.quote(market, item).unwrap_or(5);
    let _ = world.player_list_for_sale(market, item, 2, quote + 2);
    // Un NPC sveglio nel Mercato mette in vendita della verdura.
    if let Some(i) = world
        .npcs
        .iter()
        .position(|n| n.age >= 18 && n.action == sim::Action::Idle)
    {
        let id = world.npcs[i].id;
        world.npcs[i].carriage = market;
        world.npcs[i].floor = 0;
        world.npcs[i].inventory.items.add(sim::ItemKind::Verdura, 4);
        let _ = world.list_for_sale(sim::Seller::Npc(id), market, sim::ItemKind::Verdura, 4, 3);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_script(
    mut commands: Commands,
    time: Res<Time<Real>>,
    layout: Res<TrainLayout>,
    stations: Res<StationLayout>,
    mut script: ResMut<ShotScript>,
    mut sim: ResMut<Sim>,
    mut wide: ResMut<WideView>,
    mut ui_windows: (
        ResMut<InventoryWindow>,
        ResMut<ChestWindow>,
        ResMut<ChatQueue>,
        ResMut<crate::market_ui::MarketWindow>,
    ),
    mut gang_window: ResMut<crate::gang_ui::GangWindow>,
    mut body: Single<&mut Body, With<Player>>,
    mut exit: MessageWriter<AppExit>,
    mut narrator: (
        Option<ResMut<NarratorState>>,
        ResMut<ChronicleWindow>,
        ResMut<StatsWindow>,
        ResMut<IconGallery>,
    ),
    mut selected: ResMut<SelectedNpc>,
) {
    script.wait -= time.delta_secs();
    if script.wait > 0.0 {
        return;
    }
    if script.scene == 0 {
        let now = time.elapsed_secs_f64();
        let Sim { world, brain } = &mut *sim;
        if !script.real
            && let Some(state) = narrator.0.as_mut()
        {
            stage_chronicle(state, world, now);
        }
        let evening = GameTime::from_dhm(1, EVENING.0, EVENING.1);
        // Ora per ora: il Custode esamina le bozze all'ora piena e le
        // statistiche hanno uno storico.
        while world.clock < evening {
            let minutes = evening.since(world.clock).min(60);
            world.run(brain, minutes);
            if let Some(state) = narrator.0.as_mut() {
                state.sync(world, now);
            }
        }
    }
    // Scatta la scena preparata al giro precedente, e prepara la prossima
    // solo dopo un attimo (nello stesso frame chiuderebbe le sue finestre).
    if (1..=SCENES.len()).contains(&script.scene) && !script.captured {
        let (name, ..) = SCENES[script.scene - 1];
        let path = script.dir.join(format!("{name}.png"));
        commands
            .spawn(Screenshot::image(script.target.clone()))
            .observe(save_to_disk(path));
        script.captured = true;
        script.wait = 0.5;
        return;
    }
    script.captured = false;
    let Some(&(_, spot, wide_view)) = SCENES.get(script.scene) else {
        // Lascia un attimo al salvataggio dell'ultima immagine.
        if script.scene == SCENES.len() {
            script.scene += 1;
            script.wait = 1.0;
        } else {
            exit.write(AppExit::Success);
        }
        return;
    };
    let index = (0..layout.len())
        .find(|&i| layout.floors(i) > 1)
        .unwrap_or(0);
    let half_height = start_position().y - FLOOR_Y;
    let left = TrainLayout::carriage_left(index);
    let (stairs_x0, _) = TrainLayout::stairs_x(index);
    let position = match (spot, stations.cabin) {
        (Spot::Floor(floor, Some(x)), _) => Vec2::new(left + x, floor_y(floor) + half_height),
        (Spot::Floor(floor, None), _) => {
            Vec2::new(stairs_x0 + STAIRS_WIDTH / 2.0, floor_y(floor) + 50.0)
        }
        (Spot::Cabin { windows }, Some(cabin)) => {
            let Sim { world, brain } = &mut *sim;
            stage_cabin(world);
            // Qualche minuto: l'amico saluta (i saluti sono ogni 5 minuti).
            if !windows {
                world.run(brain, 5);
            }
            ui_windows.0.open = windows;
            ui_windows.1.open = windows;
            let x = TrainLayout::carriage_left(cabin.carriage.index()) + cabin.bed_x + 18.0;
            Vec2::new(x, cabin.base_y() + half_height)
        }
        (Spot::Chat, Some(cabin)) => {
            let friend = stage_cabin(&mut sim.world);
            ui_windows.0.open = false;
            ui_windows.1.open = false;
            if let Some(id) = friend {
                ui_windows.2.0.extend([
                    ChatCommand::Open(id),
                    ChatCommand::Say(sim::Intent::Greet),
                    ChatCommand::Say(sim::Intent::AskJob),
                    ChatCommand::Type("ciao, quanto costa un vestito?".to_string()),
                    ChatCommand::Say(sim::Intent::AskFavour),
                    ChatCommand::Say(sim::Intent::AskNews),
                ]);
            }
            let x = TrainLayout::carriage_left(cabin.carriage.index()) + cabin.bed_x + 18.0;
            Vec2::new(x, cabin.base_y() + half_height)
        }
        (Spot::Cabin { .. } | Spot::Chat, None) => start_position(),
        (Spot::Market, _) => {
            narrator.1.open = false;
            narrator.3.0 = false;
            ui_windows.0.open = false;
            let world = &mut sim.world;
            let market = world
                .carriages
                .iter()
                .find(|c| c.kind == sim::CarriageKind::Mercato)
                .map(|c| c.id);
            match market {
                Some(m) => {
                    stage_stalls(world, m);
                    ui_windows.3.show_stalls();
                    Vec2::new(
                        TrainLayout::carriage_left(m.index()) + 160.0,
                        floor_y(0) + half_height,
                    )
                }
                None => start_position(),
            }
        }
        (Spot::Fight, _) => {
            ui_windows.2.0.push(ChatCommand::Close);
            narrator.1.open = false;
            narrator.2.open = false;
            narrator.3.0 = false;
            selected.0 = stage_fight(&mut sim.world, index);
            Vec2::new(left + 150.0, floor_y(0) + half_height)
        }
        (Spot::Gangs { invite }, _) => {
            ui_windows.3.open = false;
            narrator.1.open = false;
            narrator.2.open = false;
            narrator.3.0 = false;
            if script.gang_friend.is_none() {
                let Sim { world, brain } = &mut *sim;
                if let Some((member, friend)) = stage_gangs(world, index) {
                    selected.0 = Some(member);
                    script.gang_friend = Some(friend);
                }
                // Qualche minuto: l'amico saluta e invita (i saluti sono ogni 5 minuti).
                world.run(brain, 6);
            }
            gang_window.open = !invite;
            if invite && let Some(friend) = script.gang_friend {
                selected.0 = None;
                ui_windows.2.0.push(ChatCommand::Open(friend));
            }
            Vec2::new(left + 150.0, floor_y(0) + half_height)
        }
        (Spot::Narrator(view), _) => {
            ui_windows.2.0.push(ChatCommand::Close);
            narrator.1.open = view == NarratorView::Chronicle;
            narrator.1.expanded_newest = 4;
            narrator.2.open = view == NarratorView::Stats;
            narrator.2.focus = (view == NarratorView::Stats).then(|| "Morale".to_string());
            narrator.3.0 = view == NarratorView::Icons;
            if view == NarratorView::Chronicle
                && !script.real
                && let Some(state) = narrator.0.as_mut()
            {
                state.toast = Some(crate::narrator_bridge::NoveltyToast {
                    text: "Novità sul treno: «Rammendatore»".to_string(),
                    shown_at: time.elapsed_secs_f64(),
                });
            }
            Vec2::new(left + 160.0, floor_y(0) + half_height)
        }
    };
    body.teleport(position);
    body.climbing = spot == Spot::Floor(0, None);
    wide.0 = wide_view;
    script.scene += 1;
    script.wait = SCENE_WAIT;
}

/// Tutte le forme in tutti i colori, e i dettagli: per controllare le icone.
fn icon_gallery(
    mut contexts: bevy_egui::EguiContexts,
    gallery: Res<IconGallery>,
    mut icons: ResMut<ItemIcons>,
) {
    use bevy_egui::egui;
    use narrator::{Appearance, Colour, Detail, Shape};
    if !gallery.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    egui::Window::new("Icone del Narratore")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("icone").spacing([4.0, 4.0]).show(ui, |ui| {
                ui.label("");
                for shape in Shape::ALL {
                    ui.label(egui::RichText::new(shape.name()).small());
                }
                ui.end_row();
                for colour in Colour::ALL {
                    ui.label(egui::RichText::new(colour.name()).small());
                    for shape in Shape::ALL {
                        let key = format!("galleria/{}/{}", shape.name(), colour.name());
                        let t = icons.get(ctx, &key, || shape_canvas(shape, colour));
                        show_icon(ui, t, 32.0);
                    }
                    ui.end_row();
                }
                ui.label(egui::RichText::new("dettagli").small());
                for (i, shape) in Shape::ALL.into_iter().enumerate() {
                    let detail = Detail::ALL[i % Detail::ALL.len()];
                    let colour = Colour::ALL[(i * 5) % Colour::ALL.len()];
                    let look = Appearance::new(shape, colour, Some(detail));
                    let t = icons.get(ctx, &format!("galleria/{}", look.key()), || {
                        icon_canvas(&look)
                    });
                    show_icon(ui, t, 32.0).on_hover_text(look.key());
                }
                ui.end_row();
            });
        });
}
