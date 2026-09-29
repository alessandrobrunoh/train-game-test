//! Interazioni del giocatore con il treno: tasto E vicino alle cose, Q per
//! scegliere cosa prendere o comprare.
//!
//! - Vicino al magazzino di una carrozza (non Mercato): prende un'unità di
//!   un oggetto (il primo del catalogo, Q per cambiare). Non si paga, ma il
//!   treno se ne accorge (evento nel registro).
//! - Vicino a un bancone o agli scaffali di un Mercato, con un mercante al
//!   lavoro: compra un oggetto (il più economico, Q per cambiare) al prezzo
//!   del Mercato.
//! - Vicino a un NPC sveglio: gli regala qualcosa che accetta (un vestito o un
//!   attrezzo che gli manca, oppure cibo se ha fame). Chi diffida del
//!   giocatore non accetta niente.
//! - Nella propria cabina: E sul letto per dormire fino al mattino (dalle
//!   20:00), E sul baule per aprirlo (vedi `cabin.rs`).
//!
//! Tutto passa dalla sim (`World::player_*`): inventario e gettoni sono in
//! `world.player`.
//!
//! Mentre la camera segue un NPC (tasto F) non si interagisce con niente.
//!
//! Sopra il bersaglio compare un piccolo suggerimento ("E: prendi una
//! razione"); dopo aver premuto E, per un attimo, l'esito.

use bevy::prelude::*;
use sim::{CarriageId, CarriageKind, ItemKind, NpcId, Regard, StationKind, World};

use crate::cabin::ChestWindow;
use crate::camera::follow_target;
use crate::player::Player;
use crate::state::{FollowNpc, NpcSprite, Sim};
use crate::stations::{CABIN_BED_WIDTH, CHEST_WIDTH, StationLayout};
use crate::storage::{STORAGE_HEIGHT, has_storage, storage_range};
use crate::train::{FLOOR_Y, TrainLayout, TrainLocation, floor_at};

/// Quanto oltre il bordo di scaffale o bancone arriva il giocatore.
const REACH: f32 = 6.0;
/// Distanza massima (x, y) tra il giocatore e un NPC per regalargli qualcosa.
const NPC_REACH: Vec2 = Vec2::new(10.0, 20.0);
/// Quanto resta visibile l'esito di un'interazione (secondi).
const FEEDBACK_SECS: f32 = 1.6;
/// Punto sopra cui compare il suggerimento: sopra l'etichetta del magazzino,
/// sopra la testa del mercante, sopra la testa dell'NPC.
const STORAGE_PROMPT_Y: f32 = STORAGE_HEIGHT + 28.0;
const COUNTER_PROMPT_Y: f32 = 34.0;
const NPC_PROMPT_ABOVE: f32 = 12.0;
const CHEST_PROMPT_Y: f32 = 26.0;
/// Riquadro del suggerimento (pixel logici dello schermo).
const PROMPT_BOX: Vec2 = Vec2::new(480.0, 40.0);
const PROMPT_FONT: f32 = 15.0;
const PROMPT_BG: Color = Color::srgba(0.05, 0.05, 0.08, 0.80);
const PROMPT_COLOR: Color = Color::srgb(1.0, 0.92, 0.55);

/// Cosa si regala, in ordine di preferenza: prima ciò che manca, poi il cibo.
const GIFTS: [ItemKind; 5] = [
    ItemKind::Vestito,
    ItemKind::Attrezzo,
    ItemKind::Razione,
    ItemKind::Te,
    ItemKind::Verdura,
];

/// Con cosa il giocatore può interagire.
#[derive(Clone, Debug, PartialEq)]
enum TargetKind {
    /// Scorte di una carrozza: gli oggetti con almeno un'unità, in ordine di
    /// catalogo (vuoto = scorte vuote); se ne prende quello scelto con Q.
    Storage {
        carriage: CarriageId,
        items: Vec<ItemKind>,
    },
    /// Bancone o scaffali di un Mercato: oggetti disponibili col prezzo, dal
    /// più economico.
    Market {
        carriage: CarriageId,
        items: Vec<(ItemKind, u32)>,
        merchant: bool,
    },
    /// Un NPC a cui regalare `item`.
    Npc {
        id: NpcId,
        name: String,
        item: ItemKind,
    },
    /// Il letto della cabina del giocatore.
    Bed,
    /// Il baule della cabina del giocatore.
    Chest,
}

#[derive(Clone, Debug, PartialEq)]
struct Target {
    kind: TargetKind,
    /// Punto del mondo sopra cui mostrare il suggerimento.
    anchor: Vec2,
}

#[derive(Resource, Default)]
struct InteractionState {
    target: Option<Target>,
    /// Oggetto scelto al Mercato (indice, modulo il numero di oggetti).
    choice: usize,
    /// Esito dell'ultima interazione, mostrato per poco al posto del suggerimento.
    feedback: Option<(String, Vec2, Timer)>,
}

#[derive(Component)]
struct PromptRoot;

#[derive(Component)]
struct PromptText;

pub struct InteractionPlugin;

impl Plugin for InteractionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InteractionState>()
            .add_systems(Startup, spawn_prompt)
            .add_systems(
                Update,
                (update_target, interact, update_prompt)
                    .chain()
                    .after(follow_target),
            );
    }
}

// --- Ricerca del bersaglio (dati puri) ---------------------------------------

/// Oggetti con almeno un'unità intera nelle scorte della carrozza, in
/// ordine di catalogo.
fn stocked_items(world: &World, carriage: CarriageId) -> Vec<ItemKind> {
    let Some(c) = world.carriage(carriage) else {
        return Vec::new();
    };
    ItemKind::ALL
        .into_iter()
        .filter(|&item| c.stock.count(item) > 0)
        .collect()
}

/// Oggetti in vendita (almeno un'unità) al Mercato, dal più economico.
fn market_items(world: &World, carriage: CarriageId) -> Vec<(ItemKind, u32)> {
    let Some(c) = world.carriage(carriage) else {
        return Vec::new();
    };
    let mut items: Vec<(ItemKind, u32)> = ItemKind::ALL
        .into_iter()
        .filter(|&item| c.stock.count(item) >= 1)
        .filter_map(|item| Some((item, world.player_price(carriage, item)?)))
        .collect();
    items.sort_by_key(|&(item, price)| (price, item));
    items
}

/// Cosa regalare a un NPC, se accetta qualcosa che il giocatore ha (chi
/// diffida del giocatore non accetta niente).
fn gift_for(world: &World, id: NpcId) -> Option<ItemKind> {
    let npc = world.npc(id)?;
    if !npc.is_awake() || npc.regard() == Regard::Wary {
        return None;
    }
    let inventory = &world.player.inventory;
    GIFTS
        .into_iter()
        .find(|&item| inventory.count(item) > 0 && npc.accepts_gift(item))
}

/// Letto o baule della cabina, se il giocatore in `player` ci è accanto
/// (il baule ha la precedenza).
fn cabin_target(layout: &StationLayout, player: Vec2) -> Option<Target> {
    let cabin = layout.cabin?;
    let index = cabin.carriage.index();
    let left = TrainLayout::carriage_left(index);
    let local = player.x - left;
    if floor_at(player.y) != cabin.floor || !(cabin.left..=cabin.right + REACH).contains(&local) {
        return None;
    }
    let base = cabin.base_y();
    if (local - cabin.chest_x).abs() <= CHEST_WIDTH / 2.0 + REACH {
        return Some(Target {
            kind: TargetKind::Chest,
            anchor: Vec2::new(left + cabin.chest_x, base + CHEST_PROMPT_Y),
        });
    }
    ((local - cabin.bed_x).abs() <= CABIN_BED_WIDTH / 2.0 + REACH).then(|| Target {
        kind: TargetKind::Bed,
        anchor: Vec2::new(left + cabin.bed_x, base + CHEST_PROMPT_Y),
    })
}

/// Il bersaglio più adatto per il giocatore in `player` (centro del corpo).
/// Scaffali e banconi hanno la precedenza sugli NPC. `npcs` sono gli NPC
/// disegnati: id e centro dello sprite.
fn find_target(
    world: &World,
    layout: &TrainLayout,
    stations: &StationLayout,
    player: Vec2,
    npcs: impl Iterator<Item = (NpcId, Vec2)>,
) -> Option<Target> {
    let TrainLocation::Carriage(index) = layout.location_at(player.x) else {
        return None;
    };
    if let Some(target) = cabin_target(stations, player) {
        return Some(target);
    }
    // Scorte e banconi sono al piano terra.
    let ground = floor_at(player.y) == 0;
    let carriage = world.carriages.get(index)?;
    let id = carriage.id;
    let left = TrainLayout::carriage_left(index);
    let local = player.x - left;

    let (s0, s1) = storage_range();
    let near_storage = ground
        && has_storage(&world.params, carriage.kind)
        && (s0 - REACH..=s1 + REACH).contains(&local);
    let storage_anchor = Vec2::new(left + (s0 + s1) / 2.0, FLOOR_Y + STORAGE_PROMPT_Y);

    if carriage.kind == CarriageKind::Mercato && ground {
        let counter = stations
            .carriages
            .get(index)
            .into_iter()
            .flatten()
            .filter(|s| s.kind == StationKind::Counter)
            .map(|s| (s, (s.x - local).abs() - s.width / 2.0))
            .filter(|&(_, gap)| gap <= REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let anchor = match counter {
            Some((spot, _)) => Some(Vec2::new(left + spot.x, spot.base_y() + COUNTER_PROMPT_Y)),
            None if near_storage => Some(storage_anchor),
            None => None,
        };
        if let Some(anchor) = anchor {
            return Some(Target {
                kind: TargetKind::Market {
                    carriage: id,
                    items: market_items(world, id),
                    merchant: world.merchant_on_duty(id).is_some(),
                },
                anchor,
            });
        }
    } else if near_storage {
        return Some(Target {
            kind: TargetKind::Storage {
                carriage: id,
                items: stocked_items(world, id),
            },
            anchor: storage_anchor,
        });
    }

    // Niente da regalare: inutile cercare NPC.
    if !GIFTS
        .iter()
        .any(|&item| world.player.inventory.count(item) > 0)
    {
        return None;
    }
    let mut near: Vec<(NpcId, Vec2)> = npcs
        .filter(|&(_, pos)| {
            let d = (pos - player).abs();
            d.x <= NPC_REACH.x && d.y <= NPC_REACH.y
        })
        .collect();
    near.sort_by(|a, b| {
        let da = (a.1.x - player.x).abs();
        let db = (b.1.x - player.x).abs();
        da.total_cmp(&db).then(a.0.cmp(&b.0))
    });
    near.into_iter().find_map(|(npc, pos)| {
        let item = gift_for(world, npc)?;
        Some(Target {
            kind: TargetKind::Npc {
                id: npc,
                name: world.npc(npc)?.name.clone(),
                item,
            },
            anchor: pos + Vec2::Y * NPC_PROMPT_ABOVE,
        })
    })
}

/// Testo del suggerimento per un bersaglio.
fn prompt_text(world: &World, target: &TargetKind, choice: usize) -> String {
    let tokens = world.player.tokens;
    match target {
        TargetKind::Bed => match world.params.is_long_sleep(world.clock.hour()) {
            true => format!(
                "E: dormi fino alle {:02}:00 (salva la partita)",
                world.params.wake_hour
            ),
            false => format!(
                "Il tuo letto (si dorme dalle {:02}:00)",
                world.params.long_sleep_from_hour
            ),
        },
        TargetKind::Chest => "E: apri il baule".to_string(),
        TargetKind::Storage { items, .. } if items.is_empty() => "Scorte vuote".to_string(),
        TargetKind::Storage { items, .. } => {
            let item = items[choice % items.len()];
            let mut text = format!("E: prendi {}", item.with_article());
            if items.len() > 1 {
                text.push_str("   Q: altro");
            }
            text
        }
        TargetKind::Market {
            merchant: false, ..
        } => "Nessun mercante al bancone".to_string(),
        TargetKind::Market { items, .. } if items.is_empty() => "Merce esaurita".to_string(),
        TargetKind::Market { items, .. } => {
            let (item, price) = items[choice % items.len()];
            let mut text = format!(
                "E: compra {} ({price} gettoni, ne hai {tokens})",
                item.with_article()
            );
            if items.len() > 1 {
                text.push_str("   Q: altro");
            }
            text
        }
        TargetKind::Npc { name, item, .. } => format!("E: dai {} a {name}", item.with_article()),
    }
}

/// Prima lettera maiuscola (per i messaggi d'errore della sim).
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Esegue l'interazione con `target`; restituisce l'esito da mostrare
/// (None: niente da dire, es. si apre il baule).
fn perform(world: &mut World, target: &TargetKind, choice: usize) -> Option<String> {
    let message = match target {
        TargetKind::Storage { items, .. } if items.is_empty() => "Le scorte sono vuote".to_string(),
        TargetKind::Storage { carriage, items } => {
            let item = items[choice % items.len()];
            if world.player.inventory.room_for(item) == 0 {
                return Some("L'inventario è pieno".to_string());
            }
            match world.player_take(*carriage, item, 1) {
                0 => "Non c'è più niente da prendere".to_string(),
                _ => format!("Preso: {}", item.with_article()),
            }
        }
        TargetKind::Market { items, .. } if items.is_empty() => "Merce esaurita".to_string(),
        TargetKind::Market {
            carriage, items, ..
        } => {
            let (item, _) = items[choice % items.len()];
            match world.player_buy(*carriage, item) {
                Ok(price) => format!("Comprato {} per {price} gettoni", item.with_article()),
                Err(e) => capitalized(&e.to_string()),
            }
        }
        TargetKind::Npc { id, name, item } => match world.player_give(*id, *item) {
            Ok(()) => format!("Hai dato {} a {name}", item.with_article()),
            Err(e) => format!("{name}: {e}"),
        },
        TargetKind::Bed => match world.player_go_to_bed() {
            Ok(wake) => format!("Buonanotte… (sveglia alle {:02}:00)", wake.hour()),
            Err(e) => capitalized(&e.to_string()),
        },
        TargetKind::Chest => return None,
    };
    Some(message)
}

// --- Sistemi ----------------------------------------------------------------

fn spawn_prompt(mut commands: Commands) {
    commands
        .spawn((
            Name::new("Suggerimento interazione"),
            PromptRoot,
            // Riquadro largo e trasparente, centrato sul bersaglio: il box
            // visibile sta in basso al centro, così non serve misurare il testo.
            Node {
                position_type: PositionType::Absolute,
                width: px(PROMPT_BOX.x),
                height: px(PROMPT_BOX.y),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::FlexEnd,
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent
                .spawn((
                    Node {
                        padding: UiRect::axes(px(6), px(3)),
                        ..default()
                    },
                    BackgroundColor(PROMPT_BG),
                ))
                .with_child((
                    PromptText,
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(PROMPT_FONT),
                        ..default()
                    },
                    TextColor(PROMPT_COLOR),
                ));
        });
}

#[allow(clippy::too_many_arguments)]
fn update_target(
    sim: Res<Sim>,
    follow: Res<FollowNpc>,
    layout: Res<TrainLayout>,
    stations: Res<StationLayout>,
    player: Single<&Transform, With<Player>>,
    npcs: Query<(&NpcSprite, &Transform), Without<Player>>,
    mut state: ResMut<InteractionState>,
) {
    // Mentre la camera segue un NPC il giocatore non interagisce.
    let target = (!follow.0)
        .then(|| {
            find_target(
                &sim.world,
                &layout,
                &stations,
                player.translation.truncate(),
                npcs.iter()
                    .map(|(npc, transform)| (npc.0, transform.translation.truncate())),
            )
        })
        .flatten();
    if state.target != target {
        state.target = target;
    }
}

/// E: interagisce con il bersaglio; Q: cambia l'oggetto da comprare.
fn interact(
    keys: Res<ButtonInput<KeyCode>>,
    mut sim: ResMut<Sim>,
    mut chest: ResMut<ChestWindow>,
    mut state: ResMut<InteractionState>,
) {
    if keys.just_pressed(KeyCode::KeyQ) {
        state.choice = state.choice.wrapping_add(1);
    }
    if !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    let Some(target) = state.target.clone() else {
        return;
    };
    if sim.world.player.is_asleep() {
        return;
    }
    if target.kind == TargetKind::Chest {
        chest.open = !chest.open;
    }
    if let Some(message) = perform(&mut sim.world, &target.kind, state.choice) {
        state.feedback = Some((
            message,
            target.anchor,
            Timer::from_seconds(FEEDBACK_SECS, TimerMode::Once),
        ));
    }
}

/// Posiziona il suggerimento (o l'esito) sopra il bersaglio, in coordinate schermo.
fn update_prompt(
    time: Res<Time>,
    sim: Res<Sim>,
    mut state: ResMut<InteractionState>,
    camera: Single<(&Camera, &Transform), With<Camera2d>>,
    root: Single<(&mut Node, &mut Visibility), With<PromptRoot>>,
    mut text: Single<&mut Text, With<PromptText>>,
) {
    if let Some((_, _, timer)) = &mut state.feedback
        && timer.tick(time.delta()).is_finished()
    {
        state.feedback = None;
    }
    let shown = match (&state.feedback, &state.target) {
        (Some((message, anchor, _)), _) => Some((message.clone(), *anchor)),
        (None, Some(target)) if !sim.world.player.is_asleep() => Some((
            prompt_text(&sim.world, &target.kind, state.choice),
            target.anchor,
        )),
        (None, _) => None,
    };

    let (mut node, mut visibility) = root.into_inner();
    let Some((label, anchor)) = shown else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    // Il Transform della camera è già aggiornato per questo frame (il
    // GlobalTransform lo sarebbe solo in PostUpdate).
    let (camera, camera_transform) = *camera;
    let Ok(point) = camera.world_to_viewport(
        &GlobalTransform::from(*camera_transform),
        anchor.extend(0.0),
    ) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    visibility.set_if_neq(Visibility::Inherited);
    let (left, top) = (px(point.x - PROMPT_BOX.x / 2.0), px(point.y - PROMPT_BOX.y));
    if node.left != left || node.top != top {
        node.left = left;
        node.top = top;
    }
    if text.0 != label {
        text.0 = label;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stations::StationLayout;

    struct Fixture {
        world: World,
        layout: TrainLayout,
        stations: StationLayout,
    }

    fn fixture() -> Fixture {
        let world = World::generate(42, 20, 400);
        let layout = TrainLayout::from_world(&world);
        let stations = StationLayout::from_world(&world);
        Fixture {
            world,
            layout,
            stations,
        }
    }

    fn first(world: &World, kind: CarriageKind) -> CarriageId {
        world.carriages.iter().find(|c| c.kind == kind).unwrap().id
    }

    fn at_storage(carriage: CarriageId) -> Vec2 {
        let (s0, s1) = storage_range();
        Vec2::new(
            TrainLayout::carriage_left(carriage.index()) + (s0 + s1) / 2.0,
            FLOOR_Y + 12.0,
        )
    }

    fn target(f: &Fixture, pos: Vec2, npcs: &[(NpcId, Vec2)]) -> Option<Target> {
        find_target(&f.world, &f.layout, &f.stations, pos, npcs.iter().copied())
    }

    fn count(f: &Fixture, item: ItemKind) -> u32 {
        f.world.player.inventory.count(item)
    }

    #[test]
    fn takes_items_from_storage() {
        let mut f = fixture();
        let officina = first(&f.world, CarriageKind::Officina);
        let t = target(&f, at_storage(officina), &[]).expect("magazzino");
        // Officina all'avvio: rottami, attrezzi, vestiti, metallo, tessuto e
        // le comodità per i Dormitori, in ordine di catalogo.
        let TargetKind::Storage { carriage, items } = &t.kind else {
            panic!("{t:?}");
        };
        assert_eq!(*carriage, officina);
        assert_eq!(items[0], ItemKind::Rottame);
        assert!(items.contains(&ItemKind::Tessuto) && items.contains(&ItemKind::Coperta));
        assert_eq!(
            prompt_text(&f.world, &t.kind, 0),
            "E: prendi un pezzo di rottame   Q: altro"
        );
        let message = perform(&mut f.world, &t.kind, 0).unwrap();
        assert!(message.starts_with("Preso"), "{message}");
        assert_eq!(count(&f, ItemKind::Rottame), 1);
        assert_eq!(
            f.world.carriages[officina.index()]
                .stock
                .count(ItemKind::Rottame),
            19
        );
        // Q: the next item.
        let tessuto = items.iter().position(|&i| i == ItemKind::Tessuto).unwrap();
        perform(&mut f.world, &t.kind, tessuto);
        assert_eq!(count(&f, ItemKind::Tessuto), 1);
        // Inventario pieno: non si prende niente.
        f.world.player.inventory.add(ItemKind::Attrezzo, 99);
        f.world.player.inventory.add(ItemKind::Rottame, 9);
        let full = perform(&mut f.world, &t.kind, 0).unwrap();
        assert_eq!(full, "L'inventario è pieno");

        // Lontano dal magazzino (e senza niente da regalare): nessun bersaglio.
        f.world.player.inventory = sim::SlotInventory::new(sim::INVENTORY_SLOTS);
        let center = Vec2::new(TrainLayout::carriage_center_x(officina.index()), 12.0);
        assert_eq!(target(&f, center, &[]), None);
        // Nei Dormitori non c'è magazzino.
        let dorm = first(&f.world, CarriageKind::Dormitorio);
        assert_eq!(target(&f, at_storage(dorm), &[]), None);
    }

    #[test]
    fn buys_at_the_market_only_with_a_merchant() {
        let mut f = fixture();
        let market = first(&f.world, CarriageKind::Mercato);
        let t = target(&f, at_storage(market), &[]).expect("mercato");
        let TargetKind::Market {
            items, merchant, ..
        } = &t.kind
        else {
            panic!("{t:?}");
        };
        assert!(!merchant);
        // Dal più economico: i vestiti costano meno degli attrezzi.
        assert_eq!(items[0].0, ItemKind::Vestito);
        assert_eq!(
            prompt_text(&f.world, &t.kind, 0),
            "Nessun mercante al bancone"
        );
        assert_eq!(
            perform(&mut f.world, &t.kind, 0).unwrap(),
            "Nessun mercante al bancone"
        );
        assert_eq!(f.world.player.tokens, sim::PLAYER_START_TOKENS);

        // Un mercante al primo bancone.
        let counter = f.world.carriages[market.index()]
            .free_station(StationKind::Counter)
            .unwrap();
        let m = f
            .world
            .npcs
            .iter()
            .position(|n| n.job == Some(sim::Job::Mercante))
            .unwrap();
        f.world.npcs[m].carriage = market;
        f.world.npcs[m].action = sim::Action::Work(counter);
        f.world.carriages[market.index()].stations[counter.index()].occupancy += 1;

        let spot = f.stations.spot(market, counter).unwrap();
        let pos = Vec2::new(TrainLayout::carriage_left(market.index()) + spot.x, 12.0);
        let t = target(&f, pos, &[]).expect("bancone");
        let price = f.world.player_price(market, ItemKind::Attrezzo).unwrap();
        // Q: il secondo oggetto è l'attrezzo.
        f.world.player.tokens = 50;
        assert_eq!(
            prompt_text(&f.world, &t.kind, 1),
            format!("E: compra un attrezzo ({price} gettoni, ne hai 50)   Q: altro")
        );
        f.world.player.tokens = 100;
        perform(&mut f.world, &t.kind, 1);
        assert_eq!(count(&f, ItemKind::Attrezzo), 1);
        assert_eq!(f.world.player.tokens, 100 - price);
        // Troppo povero per un altro attrezzo.
        f.world.player.tokens = 1;
        let message = perform(&mut f.world, &t.kind, 1).unwrap();
        assert!(message.starts_with("Servono"), "{message}");
        assert_eq!(count(&f, ItemKind::Attrezzo), 1);
    }

    #[test]
    fn gives_food_to_a_hungry_npc_nearby() {
        let mut f = fixture();
        let dorm = first(&f.world, CarriageKind::Dormitorio);
        let pos = Vec2::new(TrainLayout::carriage_center_x(dorm.index()), 12.0);
        let npc = f.world.npcs[0].id;
        f.world.npcs[0].needs.hunger = 0.2;
        f.world.npcs[0].inventory.clothes = Some(0.5);
        let npcs = [(npc, pos + Vec2::new(4.0, -4.0))];
        // Senza niente da dare, niente bersaglio.
        assert_eq!(target(&f, pos, &npcs), None);

        f.world.player.inventory.add(ItemKind::Razione, 2);
        let t = target(&f, pos, &npcs).expect("npc");
        let TargetKind::Npc { id, item, .. } = &t.kind else {
            panic!("{t:?}");
        };
        assert_eq!((*id, *item), (npc, ItemKind::Razione));
        let message = perform(&mut f.world, &t.kind, 0).unwrap();
        assert!(message.starts_with("Hai dato una razione"), "{message}");
        assert_eq!(count(&f, ItemKind::Razione), 1);
        assert!(f.world.npc(npc).unwrap().needs.hunger > 0.7);

        // Sazio: non accetta altro cibo.
        f.world.npcs[0].needs.hunger = 0.95;
        assert_eq!(target(&f, pos, &npcs), None);
        // Troppo lontano.
        f.world.npcs[0].needs.hunger = 0.2;
        let far = [(npc, pos + Vec2::new(30.0, 0.0))];
        assert_eq!(target(&f, pos, &far), None);
        // Chi diffida non accetta niente.
        f.world.npcs[0].player = Some(sim::PlayerTie {
            affinity: -0.6,
            ..sim::PlayerTie::default()
        });
        assert_eq!(target(&f, pos, &npcs), None);
    }

    #[test]
    fn the_cabin_has_a_bed_to_sleep_in_and_a_chest() {
        let mut f = fixture();
        let cabin = f.stations.cabin.expect("cabina");
        let left = TrainLayout::carriage_left(cabin.carriage.index());
        let y = cabin.base_y() + 12.0;
        let chest = target(&f, Vec2::new(left + cabin.chest_x, y), &[]).expect("baule");
        assert_eq!(chest.kind, TargetKind::Chest);
        assert_eq!(perform(&mut f.world, &chest.kind, 0), None);
        let bed = target(&f, Vec2::new(left + cabin.bed_x, y), &[]).expect("letto");
        assert_eq!(bed.kind, TargetKind::Bed);
        // Alle 06:00 è presto; alle 21:00 si dorme fino al mattino.
        assert!(prompt_text(&f.world, &bed.kind, 0).contains("dalle 20:00"));
        let early = perform(&mut f.world, &bed.kind, 0).unwrap();
        assert!(early.contains("presto"), "{early}");
        f.world.clock = sim::GameTime::from_dhm(1, 21, 0);
        let night = perform(&mut f.world, &bed.kind, 0).unwrap();
        assert!(night.contains("06:00"), "{night}");
        assert!(f.world.player.is_asleep());
        // Al piano di sotto, sotto la cabina, niente.
        let below = Vec2::new(left + cabin.bed_x, FLOOR_Y + 12.0);
        assert_ne!(
            target(&f, below, &[]).map(|t| t.kind),
            Some(TargetKind::Bed)
        );
    }
}
