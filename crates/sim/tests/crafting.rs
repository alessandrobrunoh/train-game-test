//! Oggetti e ricette (Fase 3): crafting del giocatore, produzione degli NPC
//! con le stesse ricette, comodità nei Dormitori e alla Mensa.

use sim::{
    CarriageId, CarriageKind, CraftError, ItemKind, MINUTES_PER_DAY, RECIPES, RecipeDef,
    StationKind, Stock, UtilityBrain, World,
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

#[test]
fn player_crafts_at_the_right_station_using_exactly_the_inputs() {
    let mut w = World::generate(7, 10, 60);
    let officina = first_of(&w, CarriageKind::Officina);
    let before = w.clone();
    let mut items = Stock::default();
    items.set(ItemKind::Tessuto, 3.0);
    items.set(ItemKind::Verdura, 1.0);

    // Coperta: 2 Tessuto → 1 Coperta, at a workbench.
    assert_eq!(
        w.player_craft(recipe("coperta"), &mut items, officina),
        Ok(1)
    );
    assert_eq!(items.count(ItemKind::Tessuto), 1);
    assert_eq!(items.count(ItemKind::Coperta), 1);
    assert_eq!(
        items.count(ItemKind::Verdura),
        1,
        "unrelated items untouched"
    );
    // Crafting uses no world randomness and changes no carriage.
    assert_eq!(
        serde_json::to_string(&w).unwrap(),
        serde_json::to_string(&before).unwrap()
    );

    // A multi-input recipe uses every input.
    items.set(ItemKind::Metallo, 1.0);
    items.set(ItemKind::Rottame, 1.0);
    assert_eq!(
        w.player_craft(recipe("lampada"), &mut items, officina),
        Ok(1)
    );
    assert_eq!(items.count(ItemKind::Metallo), 0);
    assert_eq!(items.count(ItemKind::Rottame), 0);
    assert_eq!(items.count(ItemKind::Lampada), 1);

    // A batch can make more than one unit (Tè: a mazzo di erbe, 3 pots).
    let mensa = first_of(&w, CarriageKind::Mensa);
    items.set(ItemKind::Erbe, 1.0);
    let te = recipe("te");
    assert_eq!(w.player_craft(te, &mut items, mensa), Ok(te.batch));
    assert_eq!(items.count(ItemKind::Te), te.batch);
}

#[test]
fn player_craft_fails_without_station_or_inputs_and_changes_nothing() {
    let mut w = World::generate(7, 10, 60);
    let officina = first_of(&w, CarriageKind::Officina);
    let mensa = first_of(&w, CarriageKind::Mensa);
    let mut items = Stock::default();
    items.set(ItemKind::Tessuto, 1.5);
    let kept = items;

    assert_eq!(
        w.player_craft(recipe("coperta"), &mut items, mensa),
        Err(CraftError::WrongPlace(StationKind::Workbench))
    );
    assert_eq!(
        w.player_craft(recipe("coperta"), &mut items, officina),
        Err(CraftError::Missing {
            item: ItemKind::Tessuto,
            needed: 2,
            have: 1,
        })
    );
    // Missing the second input of a two-input recipe: the first stays too.
    items.set(ItemKind::Metallo, 1.0);
    let with_metal = items;
    assert!(matches!(
        w.player_craft(recipe("lampada"), &mut items, officina),
        Err(CraftError::Missing {
            item: ItemKind::Rottame,
            ..
        })
    ));
    assert_eq!(items, with_metal);
    items = kept;
    assert_eq!(
        w.player_craft(recipe("coperta"), &mut items, CarriageId(999)),
        Err(CraftError::NoSuchCarriage)
    );
    assert_eq!(items, kept);
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
