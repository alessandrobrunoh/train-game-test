//! Magazzini delle carrozze (solo grafica).
//!
//! Ogni carrozza con scorte (`Carriage::stock`) ha una zona magazzino vicino
//! alla testata destra: uno scaffale con una griglia di casse. Le colonne sono
//! divise tra gli oggetti che la carrozza può tenere e le casse visibili sono
//! proporzionali a scorta / capienza, colorate per tipo di oggetto. Sopra c'è
//! un'etichetta con le quantità (solo quelle non nulle) e, nei Mercati, i
//! prezzi; sui banconi dei Mercati è esposta la merce disponibile.
//!
//! Tutto viene creato all'avvio e poi aggiornato poche volte al secondo, solo
//! per le carrozze della finestra visibile, cambiando visibilità e testo.

use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy::sprite::Text2dShadow;
use bevy::text::Justify;
use sim::{CarriageId, CarriageKind, ItemKind, SimParams, StationKind, World};

use crate::npc_render::visible_window;
use crate::state::Sim;
use crate::stations::{StationLayout, interior_range};
use crate::train::{FLOOR_Y, TrainLayout};

/// Larghezza della zona magazzino (a ridosso della testata destra).
pub const STORAGE_WIDTH: f32 = 40.0;
/// Altezza dello scaffale.
pub const STORAGE_HEIGHT: f32 = 44.0;
/// Griglia di casse dello scaffale.
const COLUMNS: usize = 4;
const ROWS: usize = 5;
/// Bordo dello scaffale attorno alle casse e spazio tra una cassa e l'altra.
const PAD: f32 = 1.5;
const CRATE_GAP: f32 = 1.0;
/// Ogni quanti secondi reali aggiornare casse ed etichette.
const REFRESH_SECS: f32 = 0.25;

/// Altezza del piano dei banconi (vedi `stations.rs`).
const COUNTER_TOP: f32 = 14.0;

// Profondità: lo scaffale sta dietro alle postazioni e agli NPC.
const Z_SHELF: f32 = -4.0;
const Z_CRATE: f32 = -3.9;
const Z_GOODS: f32 = -2.9;
const Z_LABEL: f32 = 5.0;

const SHELF_COLOR: Color = Color::srgb(0.22, 0.16, 0.11);
const LABEL_COLOR: Color = Color::srgb(0.98, 0.95, 0.85);
const LABEL_SHADOW: Color = Color::srgba(0.0, 0.0, 0.0, 0.85);

/// Intervallo x (locale alla carrozza) della zona magazzino.
pub fn storage_range() -> (f32, f32) {
    let (_, right) = interior_range();
    (right - STORAGE_WIDTH, right)
}

/// Oggetti che una carrozza di tipo `kind` tiene in magazzino, in ordine.
pub fn storable_items(params: &SimParams, kind: CarriageKind) -> Vec<ItemKind> {
    ItemKind::ALL
        .into_iter()
        .filter(|&item| params.storage_cap(kind, item) > 0.0)
        .collect()
}

/// Vero se la carrozza ha un magazzino (i Dormitori no).
pub fn has_storage(params: &SimParams, kind: CarriageKind) -> bool {
    ItemKind::ALL
        .into_iter()
        .any(|item| params.storage_cap(kind, item) > 0.0)
}

/// Colore di un oggetto (casse, merce, interfaccia).
pub fn item_color(item: ItemKind) -> Color {
    match item {
        ItemKind::Verdura => Color::srgb(0.40, 0.72, 0.26),
        ItemKind::Razione => Color::srgb(0.90, 0.70, 0.38),
        ItemKind::Rottame => Color::srgb(0.55, 0.40, 0.32),
        ItemKind::Attrezzo => Color::srgb(0.62, 0.68, 0.78),
        ItemKind::Vestito => Color::srgb(0.78, 0.32, 0.38),
    }
}

/// "Razioni", "Attrezzi", ...
pub fn plural_title(item: ItemKind) -> String {
    let plural = item.plural();
    let mut chars = plural.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Colonne dello scaffale per ogni oggetto: divise in parti uguali, le
/// eccedenti al primo (es. Officina: rottame 2, attrezzi 1, vestiti 1).
fn columns_for(items: &[ItemKind]) -> Vec<(ItemKind, usize)> {
    if items.is_empty() {
        return Vec::new();
    }
    let each = (COLUMNS / items.len()).max(1);
    let mut columns: Vec<(ItemKind, usize)> = items.iter().map(|&item| (item, each)).collect();
    let used = each * items.len();
    if used < COLUMNS {
        columns[0].1 += COLUMNS - used;
    }
    columns
}

/// Casse piene da mostrare per una scorta: proporzionali a scorta / capienza,
/// almeno una se c'è un'unità intera, nessuna altrimenti.
fn crates_shown(amount: f32, cap: f32, slots: usize) -> usize {
    if amount < 1.0 || cap <= 0.0 || slots == 0 {
        return 0;
    }
    ((amount / cap * slots as f32).ceil() as usize).clamp(1, slots)
}

/// Testo dell'etichetta: una riga per oggetto presente, con il prezzo nei Mercati.
fn label_text(world: &World, carriage: CarriageId) -> String {
    let Some(c) = world.carriage(carriage) else {
        return String::new();
    };
    let mut lines = Vec::new();
    for item in storable_items(&world.params, c.kind) {
        let count = c.stock.count(item);
        if count == 0 {
            continue;
        }
        let mut line = format!("{} {count}", plural_title(item));
        if let Some(price) = world.price(carriage, item) {
            line.push_str(&format!(" · {price} g"));
        }
        lines.push(line);
    }
    if lines.is_empty() {
        "Scorte vuote".to_string()
    } else {
        lines.join("\n")
    }
}

/// Una cassa dello scaffale: visibile se l'oggetto riempie più di `rank` casse.
#[derive(Component)]
struct Crate {
    carriage: usize,
    item: ItemKind,
    rank: usize,
    /// Casse disponibili per l'oggetto.
    slots: usize,
}

/// Etichetta delle quantità sopra lo scaffale.
#[derive(Component)]
struct StorageLabel {
    carriage: usize,
}

/// Merce esposta su un bancone del Mercato: visibile se l'oggetto è in vendita.
#[derive(Component)]
struct CounterGood {
    carriage: usize,
    item: ItemKind,
}

#[derive(Resource)]
struct StorageRefresh(Timer);

pub struct StoragePlugin;

impl Plugin for StoragePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(StorageRefresh(Timer::from_seconds(
            REFRESH_SECS,
            TimerMode::Repeating,
        )))
        .add_systems(Startup, spawn_storage)
        .add_systems(Update, refresh_storage);
    }
}

/// Crea scaffali, casse, etichette e merce sui banconi di ogni carrozza.
fn spawn_storage(mut commands: Commands, sim: Res<Sim>, stations: Res<StationLayout>) {
    let world = &sim.world;
    let (x0, x1) = storage_range();
    let crate_w = (STORAGE_WIDTH - 2.0 * PAD - (COLUMNS - 1) as f32 * CRATE_GAP) / COLUMNS as f32;
    let crate_h = (STORAGE_HEIGHT - 2.0 * PAD - (ROWS - 1) as f32 * CRATE_GAP) / ROWS as f32;

    for c in &world.carriages {
        let index = c.id.index();
        let items = storable_items(&world.params, c.kind);
        if items.is_empty() {
            continue;
        }
        let origin = Vec2::new(TrainLayout::carriage_left(index), FLOOR_Y);
        commands
            .spawn((
                Name::new(format!("Magazzino carrozza {}", c.id)),
                Transform::from_translation(origin.extend(0.0)),
                Visibility::default(),
            ))
            .with_children(|parent| {
                // Scaffale
                parent.spawn((
                    Sprite::from_color(SHELF_COLOR, Vec2::new(STORAGE_WIDTH, STORAGE_HEIGHT)),
                    Transform::from_xyz((x0 + x1) / 2.0, STORAGE_HEIGHT / 2.0, Z_SHELF),
                ));

                // Casse: ogni oggetto ha le sue colonne, riempite dal basso una
                // riga alla volta (così sembra una pila).
                let mut first_col = 0;
                for (item, cols) in columns_for(&items) {
                    let slots = cols * ROWS;
                    let cap = world.params.storage_cap(c.kind, item);
                    let shown = crates_shown(c.stock.get(item), cap, slots);
                    let base = item_color(item).to_srgba();
                    for rank in 0..slots {
                        let (row, col) = (rank / cols, first_col + rank % cols);
                        // Casse alterne un po' più scure: si distinguono meglio.
                        let shade = if (row + col) % 2 == 0 { 1.0 } else { 0.85 };
                        let color =
                            Color::srgb(base.red * shade, base.green * shade, base.blue * shade);
                        let x = x0 + PAD + col as f32 * (crate_w + CRATE_GAP) + crate_w / 2.0;
                        let y = PAD + row as f32 * (crate_h + CRATE_GAP) + crate_h / 2.0;
                        parent.spawn((
                            Crate {
                                carriage: index,
                                item,
                                rank,
                                slots,
                            },
                            Sprite::from_color(color, Vec2::new(crate_w, crate_h)),
                            Transform::from_xyz(x, y, Z_CRATE),
                            if rank < shown {
                                Visibility::Inherited
                            } else {
                                Visibility::Hidden
                            },
                        ));
                    }
                    first_col += cols;
                }

                // Etichetta, allineata a destra sopra lo scaffale. Font grande
                // e scala ridotta come il nome della carrozza.
                parent.spawn((
                    StorageLabel { carriage: index },
                    Text2d::new(label_text(world, c.id)),
                    TextFont {
                        font_size: FontSize::Px(40.0),
                        ..default()
                    },
                    TextColor(LABEL_COLOR),
                    TextLayout::justify(Justify::Right),
                    Text2dShadow {
                        offset: Vec2::new(3.0, -3.0),
                        color: LABEL_SHADOW,
                    },
                    Anchor::BOTTOM_RIGHT,
                    Transform::from_xyz(x1, STORAGE_HEIGHT + 2.0, Z_LABEL)
                        .with_scale(Vec3::splat(0.18)),
                ));

                // Merce sui banconi del Mercato: un attrezzo e un vestito.
                if c.kind != CarriageKind::Mercato {
                    return;
                }
                let spots = stations.carriages.get(index).map_or(&[][..], Vec::as_slice);
                for spot in spots.iter().filter(|s| s.kind == StationKind::Counter) {
                    let w = spot.width;
                    for (item, dx, size) in [
                        (
                            ItemKind::Attrezzo,
                            -w / 4.0,
                            Vec2::new((w / 4.0).min(6.0), 1.5),
                        ),
                        (
                            ItemKind::Vestito,
                            w / 4.0,
                            Vec2::new((w / 5.0).min(5.0), 3.0),
                        ),
                    ] {
                        let visible = c.stock.count(item) >= 1;
                        parent.spawn((
                            CounterGood {
                                carriage: index,
                                item,
                            },
                            Sprite::from_color(item_color(item), size),
                            Transform::from_xyz(
                                spot.x + dx,
                                spot.base_y() - FLOOR_Y + COUNTER_TOP + size.y / 2.0,
                                Z_GOODS,
                            ),
                            if visible {
                                Visibility::Inherited
                            } else {
                                Visibility::Hidden
                            },
                        ));
                    }
                }
            });
    }
}

/// Poche volte al secondo: casse, etichette e merce delle carrozze visibili.
fn refresh_storage(
    time: Res<Time<Real>>,
    mut refresh: ResMut<StorageRefresh>,
    sim: Res<Sim>,
    camera: Single<(&Transform, &Projection), With<Camera2d>>,
    mut crates: Query<(&Crate, &mut Visibility), Without<CounterGood>>,
    mut goods: Query<(&CounterGood, &mut Visibility)>,
    mut labels: Query<(&StorageLabel, &mut Text2d)>,
) {
    if !refresh.0.tick(time.delta()).just_finished() {
        return;
    }
    let world = &sim.world;
    let (camera_transform, projection) = *camera;
    let Projection::Orthographic(ortho) = projection else {
        return;
    };
    let (lo, hi) = visible_window(
        camera_transform.translation.x,
        ortho.area,
        world.carriages.len(),
    );
    let visible = |i: usize| (lo..=hi).contains(&i);

    for (c, mut visibility) in &mut crates {
        if !visible(c.carriage) {
            continue;
        }
        let Some(carriage) = world.carriages.get(c.carriage) else {
            continue;
        };
        let cap = world.params.storage_cap(carriage.kind, c.item);
        let shown = crates_shown(carriage.stock.get(c.item), cap, c.slots);
        visibility.set_if_neq(if c.rank < shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }

    for (good, mut visibility) in &mut goods {
        if !visible(good.carriage) {
            continue;
        }
        let in_stock = world
            .carriages
            .get(good.carriage)
            .is_some_and(|c| c.stock.count(good.item) >= 1);
        visibility.set_if_neq(if in_stock {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }

    for (label, mut text) in &mut labels {
        if !visible(label.carriage) {
            continue;
        }
        let new = label_text(world, CarriageId(label.carriage as u16));
        // Scrive solo se cambia, per non rifare il layout del testo.
        if text.0 != new {
            text.0 = new;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_shared_among_items() {
        use ItemKind::*;
        let total = |cols: &[(ItemKind, usize)]| cols.iter().map(|c| c.1).sum::<usize>();
        let one = columns_for(&[Razione]);
        assert_eq!(one, vec![(Razione, COLUMNS)]);
        let two = columns_for(&[Attrezzo, Vestito]);
        assert_eq!(total(&two), COLUMNS);
        assert_eq!(two[0].1, two[1].1);
        let three = columns_for(&[Rottame, Attrezzo, Vestito]);
        assert_eq!(total(&three), COLUMNS);
        assert!(three.iter().all(|c| c.1 >= 1));
        assert!(columns_for(&[]).is_empty());
    }

    #[test]
    fn crates_follow_stock_over_capacity() {
        assert_eq!(crates_shown(0.0, 400.0, 20), 0);
        assert_eq!(crates_shown(0.9, 400.0, 20), 0);
        assert_eq!(crates_shown(1.0, 400.0, 20), 1);
        assert_eq!(crates_shown(200.0, 400.0, 20), 10);
        assert_eq!(crates_shown(400.0, 400.0, 20), 20);
        assert_eq!(crates_shown(999.0, 400.0, 20), 20);
    }

    #[test]
    fn labels_list_stock_and_market_prices() {
        let world = World::generate(42, 20, 400);
        let find = |kind| world.carriages.iter().find(|c| c.kind == kind).unwrap().id;
        let mensa = label_text(&world, find(CarriageKind::Mensa));
        assert_eq!(mensa, "Razioni 100");
        let market = find(CarriageKind::Mercato);
        let price = world.price(market, ItemKind::Attrezzo).unwrap();
        let text = label_text(&world, market);
        assert!(text.contains(&format!("Attrezzi 10 · {price} g")), "{text}");
        assert!(text.contains("Vestiti 10"), "{text}");
    }

    #[test]
    fn only_dormitories_have_no_storage() {
        let params = SimParams::default();
        for kind in CarriageKind::ALL {
            assert_eq!(has_storage(&params, kind), kind != CarriageKind::Dormitorio);
        }
    }
}
