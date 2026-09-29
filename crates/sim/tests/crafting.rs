//! Oggetti e ricette (Fase 3): crafting del giocatore, produzione degli NPC
//! con le stesse ricette, comodità nei Dormitori e alla Mensa.

use sim::{
    CarriageId, CarriageKind, CraftError, INVENTORY_SLOTS, ItemKind, MINUTES_PER_DAY, RECIPES,
    RecipeDef, SlotInventory, StationKind, UtilityBrain, World,
};

const DAY: u64 = MINUTES_PER_DAY;

fn first_of(w: &World, kind: CarriageKind) -> CarriageId {
    w.carriages
        .iter()
        .find(|c| c.kind == kind)
        .expect("carriage of this kind")
        .id
}

fn recipe(key: &str) -> &'static RecipeDef {
    RecipeDef::by_key(key).expect("recipe")
}

/// Gives the player exactly `items` (whole units).
fn give(w: &mut World, items: &[(ItemKind, u32)]) {
    w.player.inventory = SlotInventory::new(INVENTORY_SLOTS);
    for &(item, n) in items {
        assert_eq!(w.player.inventory.add(item, n), n);
    }
}

#[test]
fn player_crafts_at_the_right_station_using_exactly_the_inputs() {
    let mut w = World::generate(7, 10, 60);
    let officina = first_of(&w, CarriageKind::Officina);
    give(&mut w, &[(ItemKind::Tessuto, 3), (ItemKind::Verdura, 1)]);
    let carriages = w.carriages.clone();
    let inv = |w: &World, item| w.player.inventory.count(item);

    // Coperta: 2 Tessuto → 1 Coperta, at a workbench.
    assert_eq!(w.player_craft(recipe("coperta"), officina), Ok(1));
    assert_eq!(inv(&w, ItemKind::Tessuto), 1);
    assert_eq!(inv(&w, ItemKind::Coperta), 1);
    assert_eq!(inv(&w, ItemKind::Verdura), 1, "unrelated items untouched");
    // Crafting changes no carriage.
    assert_eq!(w.carriages, carriages);

    // A multi-input recipe uses every input (Lampada has to be learnt first).
    w.player.inventory.add(ItemKind::Metallo, 1);
    w.player.inventory.add(ItemKind::Rottame, 1);
    assert_eq!(
        w.player_craft(recipe("lampada"), officina),
        Err(CraftError::Unknown)
    );
    assert!(w.player.learn(recipe("lampada")));
    assert!(!w.player.learn(recipe("lampada")));
    assert_eq!(w.player_craft(recipe("lampada"), officina), Ok(1));
    assert_eq!(inv(&w, ItemKind::Metallo), 0);
    assert_eq!(inv(&w, ItemKind::Rottame), 0);
    assert_eq!(inv(&w, ItemKind::Lampada), 1);

    // A batch can make more than one unit (Tè: a mazzo di erbe, 3 pots).
    let mensa = first_of(&w, CarriageKind::Mensa);
    w.player.inventory.add(ItemKind::Erbe, 1);
    let te = recipe("te");
    assert_eq!(w.player_craft(te, mensa), Ok(te.batch));
    assert_eq!(inv(&w, ItemKind::Te), te.batch);
}

#[test]
fn player_craft_fails_without_station_or_inputs_and_changes_nothing() {
    let mut w = World::generate(7, 10, 60);
    let officina = first_of(&w, CarriageKind::Officina);
    let mensa = first_of(&w, CarriageKind::Mensa);
    give(&mut w, &[(ItemKind::Tessuto, 1)]);
    let kept = w.player.inventory.clone();

    assert_eq!(
        w.player_craft(recipe("coperta"), mensa),
        Err(CraftError::WrongPlace(StationKind::Workbench))
    );
    assert_eq!(
        w.player_craft(recipe("coperta"), officina),
        Err(CraftError::Missing {
            item: ItemKind::Tessuto,
            needed: 2,
            have: 1,
        })
    );
    // Missing the second input of a two-input recipe: the first stays too.
    w.player.learn(recipe("lampada"));
    w.player.inventory.add(ItemKind::Metallo, 1);
    let with_metal = w.player.inventory.clone();
    assert!(matches!(
        w.player_craft(recipe("lampada"), officina),
        Err(CraftError::Missing {
            item: ItemKind::Rottame,
            ..
        })
    ));
    assert_eq!(w.player.inventory, with_metal);
    w.player.inventory = kept.clone();
    assert_eq!(
        w.player_craft(recipe("coperta"), CarriageId(999)),
        Err(CraftError::NoSuchCarriage)
    );
    assert_eq!(w.player.inventory, kept);
    assert!(
        !CraftError::Missing {
            item: ItemKind::Tessuto,
            needed: 2,
            have: 1
        }
        .to_string()
        .is_empty()
    );
}

#[test]
fn a_full_inventory_blocks_the_result_and_keeps_the_ingredients() {
    let mut w = World::generate(7, 10, 60);
    let officina = first_of(&w, CarriageKind::Officina);
    // 2 Tessuto in their own slot, the other slots full of Attrezzi (one each).
    give(&mut w, &[(ItemKind::Tessuto, 2)]);
    w.player
        .inventory
        .add(ItemKind::Attrezzo, INVENTORY_SLOTS as u32);
    assert_eq!(w.player.inventory.free_slots(), 0);
    // The Tessuto slot empties, and the Coperta takes it.
    assert_eq!(w.player_craft(recipe("coperta"), officina), Ok(1));
    // With 4 Tessuto in one slot, using 2 frees no slot: no room.
    give(&mut w, &[(ItemKind::Tessuto, 4)]);
    w.player
        .inventory
        .add(ItemKind::Attrezzo, INVENTORY_SLOTS as u32);
    let kept = w.player.inventory.clone();
    assert_eq!(
        w.player_craft(recipe("coperta"), officina),
        Err(CraftError::NoRoom)
    );
    assert_eq!(w.player.inventory, kept);
}

#[test]
fn recipes_available_by_carriage() {
    let w = World::generate(7, 10, 60);
    let keys = |kind| -> Vec<&str> {
        w.recipes_at(first_of(&w, kind))
            .iter()
            .map(|r| r.key)
            .collect()
    };
    assert_eq!(keys(CarriageKind::Serra), ["verdura", "cotone", "erbe"]);
    assert_eq!(keys(CarriageKind::Mensa), ["razione", "te"]);
    let officina = keys(CarriageKind::Officina);
    assert!(officina.contains(&"tessuto") && officina.contains(&"giocattolo"));
    assert!(keys(CarriageKind::Dormitorio).is_empty());
    assert!(keys(CarriageKind::Mercato).is_empty());
    // Every recipe can be made somewhere on the default train.
    for r in &RECIPES {
        assert!(
            w.carriages
                .iter()
                .any(|c| w.recipes_at(c.id).iter().any(|x| x.key == r.key)),
            "{} can't be made",
            r.key
        );
    }
}

#[test]
fn workers_make_every_item_and_the_dormitori_get_comfort() {
    let mut w = World::generate(42, 20, 400);
    let mut brain = UtilityBrain::new(42);
    // Empty Dormitori: the Officine have to supply them.
    for c in w.carriages.iter_mut() {
        if c.kind == CarriageKind::Dormitorio {
            for item in [ItemKind::Coperta, ItemKind::Lampada, ItemKind::Giocattolo] {
                c.stock.set(item, 0.0);
            }
        }
    }
    w.run(&mut brain, 30 * DAY);
    let made = &w.economy.counters;
    for r in &RECIPES {
        assert!(made.made(r.output) > 1.0, "nobody made any {}", r.key);
    }
    // Tè is drunk: far more was brewed than the Mense can hold.
    let te_cap: f32 = w
        .carriages
        .iter()
        .map(|c| w.params.storage_cap(c.kind, ItemKind::Te))
        .sum();
    assert!(made.made(ItemKind::Te) > 5.0 * f64::from(te_cap));
    for c in w
        .carriages
        .iter()
        .filter(|c| c.kind == CarriageKind::Dormitorio)
    {
        let comfort = w.comfort(c.id);
        assert!(
            comfort.bedding > 0.8 && comfort.light > 0.8 && comfort.toys > 0.8,
            "{}: {comfort:?}",
            c.name
        );
        assert_eq!(
            w.furnishing_target(c.id, ItemKind::Coperta).round(),
            (w.residents(c.id) as f32 * w.params.coperte_per_resident).round()
        );
    }
    // Food is not sacrificed for the side products.
    assert_eq!(
        w.life.deaths_by_cause[sim::DeathCause::Starvation.index()],
        0
    );
    let per_person = w.available(ItemKind::Razione) / w.npcs.len() as f32;
    assert!(per_person > 1.0, "{per_person} razioni per person");
}

#[test]
fn comfort_goods_help_sleep_and_company() {
    // The same train with and without comfort goods: better energy and
    // sociality with them.
    let run = |comfort: bool| {
        let mut params = sim::SimParams::default();
        if !comfort {
            params.te_per_meal = 0.0;
            params.coperte_per_resident = 0.0;
            params.lampade_per_resident = 0.0;
            params.giocattoli_per_child = 0.0;
        }
        let mut w = World::generate_with_params(42, 10, 150, params);
        let mut brain = UtilityBrain::new(42);
        let (mut energy, mut social) = (0.0, 0.0);
        for _ in 0..5 * 24 {
            w.run(&mut brain, 60);
            let s = sim::Stats::of(&w);
            energy += s.avg_needs.energy;
            social += s.avg_needs.social;
        }
        (energy, social)
    };
    let (with, without) = (run(true), run(false));
    assert!(with.0 > without.0, "energy {with:?} vs {without:?}");
    assert!(with.1 > without.1, "social {with:?} vs {without:?}");
}

#[test]
fn production_is_deterministic() {
    let run = || {
        let mut w = World::generate(9, 12, 150);
        w.run(&mut UtilityBrain::new(9), 4 * DAY);
        serde_json::to_string(&w).unwrap()
    };
    assert_eq!(run(), run());
}
