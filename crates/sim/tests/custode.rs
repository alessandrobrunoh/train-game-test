//! Il Custode (A2): le proposte del Narratore diventano reali.

use sim::custode::{Category, Draft, Effect, Ingredient, Need, Proposal};
use sim::{
    Buyer, CarriageId, CarriageKind, GameTime, INVENTORY_SLOTS, ItemCategory, ItemKind, ItemUse,
    Job, MINUTES_PER_DAY, Place, SlotInventory, UtilityBrain, Work, World,
};

const DAY: u64 = MINUTES_PER_DAY;

fn first_of(w: &World, kind: CarriageKind) -> CarriageId {
    w.carriages.iter().find(|c| c.kind == kind).unwrap().id
}

fn draft(p: Proposal) -> Draft {
    Draft::new("Il treno ne ha bisogno.", p)
}

fn ingredients(list: &[(&str, u32)]) -> Vec<Ingredient> {
    list.iter()
        .map(|&(item, qty)| Ingredient {
            item: item.into(),
            qty,
        })
        .collect()
}

fn item(name: &str, job: &str, category: Category, value: u32, from: &[(&str, u32)]) -> Draft {
    draft(Proposal::NewItem {
        name: name.into(),
        description: "Una cosa nuova e utile per il treno.".into(),
        category,
        base_value: value,
        stack_limit: 5,
        made_from: ingredients(from),
        made_by_job: job.into(),
        appearance: Some(sim::custode::Appearance {
            shape: "bottiglia".into(),
            colour: "verde".into(),
            detail: Some("etichetta".into()),
        }),
    })
}

fn job(name: &str, kind: &str, makes: &[&str], service: Option<Need>) -> Draft {
    draft(Proposal::NewJob {
        name: name.into(),
        description: "Un mestiere che al treno mancava.".into(),
        workplace_kind: kind.into(),
        makes: makes.iter().map(|m| m.to_string()).collect(),
        service,
    })
}

fn recipe(name: &str, output: &str, qty: u32, from: &[(&str, u32)], job: &str) -> Draft {
    draft(Proposal::NewRecipe {
        name: name.into(),
        output: output.into(),
        output_qty: qty,
        inputs: ingredients(from),
        job: job.into(),
        minutes: 40,
    })
}

fn postcard_roundtrip(w: &World) -> World {
    let bytes = postcard::to_stdvec(w).expect("postcard");
    postcard::from_bytes(&bytes).expect("postcard back")
}

fn json(w: &World) -> String {
    serde_json::to_string(w).unwrap()
}

#[test]
fn a_new_item_is_stocked_crafted_listed_bought_and_saved() {
    let mut w = World::generate(7, 10, 80);
    let mut brain = UtilityBrain::new(7);
    let now = w.clock;
    let applied = w
        .apply(
            &item(
                "Borraccia",
                "operai",
                Category::Durable,
                9,
                &[("metallo", 1)],
            ),
            now,
        )
        .expect("applied");
    let soap = applied.item.expect("an item");
    assert_eq!(soap.name(), "borraccia");
    assert_eq!(soap.plural(), "borracce");
    assert_eq!(soap.usage(), ItemUse::Material);
    let def = w.catalog().item(soap).clone();
    assert_eq!(def.base_value, 9);
    assert_eq!(def.category, ItemCategory::Durable);
    assert!(def.desired);
    assert_eq!(def.appearance.as_ref().unwrap().shape, "bottiglia");
    assert!(w.storage_cap(CarriageKind::Officina, soap) > 0.0);
    assert!(w.catalog().problems(&w.params).is_empty());
    // The Operai make it among their goods.
    let recipe = applied.recipe.unwrap();
    assert_eq!(w.catalog().maker(recipe), Some(Job::Operaio));
    assert!(w.applied().count() == 1 && w.decisions()[0].at == now);

    // The player crafts one at a workbench.
    let officina = first_of(&w, CarriageKind::Officina);
    w.player.inventory = SlotInventory::new(INVENTORY_SLOTS);
    w.player.inventory.add(ItemKind::Metallo, 2);
    assert_eq!(w.player_craft(recipe, officina), Ok(1));
    assert_eq!(w.player.inventory.count(soap), 1);

    // It stacks, and the player lists it at a Mercato, where an NPC buys it.
    let market = first_of(&w, CarriageKind::Mercato);
    w.set_player_place(Place {
        carriage: market,
        floor: 0,
    });
    let quote = w.quote(market, soap).expect("a quote");
    assert!(quote >= 1);
    let listing = w
        .player_list_for_sale(market, soap, 1, quote)
        .expect("listed");
    assert!(w.listings_at(market).iter().any(|l| l.id == listing));
    let buyer = w.npcs[0].id;
    w.npcs[0].carriage = market;
    w.npcs[0].inventory.tokens = 100;
    w.npcs[0].inventory.items = sim::SlotInventory::new(sim::NPC_ITEM_SLOTS);
    // Money moves between pockets from here on: none is created.
    let money = w.money_supply();
    let tokens = w.player.tokens;
    let sale = w
        .buy_listing(Buyer::Npc(buyer), listing, 1)
        .expect("bought");
    assert_eq!(
        w.player.tokens,
        tokens + quote,
        "{sale:?} quote {quote} tokens {tokens}"
    );
    assert_eq!(w.npc(buyer).unwrap().inventory.items.count(soap), 1);
    assert_eq!(w.money_supply(), money);

    // Workers make it; it survives the saves (JSON and postcard) and the
    // saved world goes on like the original.
    w.run(&mut brain, 6 * DAY);
    assert!(w.economy.counters.made(soap) > 0.0, "nobody made any");
    assert!(w.total_stock().get(soap) > 0.0 || w.trade_pool_units() > 0);
    assert_eq!(w.money_supply(), money);
    let mut b: World = serde_json::from_str(&json(&w)).unwrap();
    let mut c = postcard_roundtrip(&w);
    assert_eq!(b.catalog().item(soap).kind.plural(), "borracce");
    assert_eq!(c.catalog().item_count(), w.catalog().item_count());
    let (mut ba, mut bb, mut bc) = (brain.clone(), brain.clone(), brain.clone());
    w.run(&mut ba, DAY);
    b.run(&mut bb, DAY);
    c.run(&mut bc, DAY);
    assert_eq!(json(&w), json(&b));
    assert_eq!(json(&w), json(&c));
}

#[test]
fn a_new_job_gets_workers_and_produces() {
    let mut w = World::generate(11, 20, 250);
    let mut brain = UtilityBrain::new(11);
    let money = w.money_supply();
    let now = w.clock;
    let soap = w
        .apply(
            &item("Sapone", "operaio", Category::Consumable, 4, &[("erbe", 1)]),
            now,
        )
        .expect("item")
        .item
        .unwrap();
    // A Consumable is food: it feeds.
    assert_eq!(soap.usage(), ItemUse::Food);
    let stations_before: usize = w.carriages.iter().map(|c| c.stations.len()).sum();
    let saponaio = w
        .apply(&job("Saponaio", "Officine", &["sapone"], None), now)
        .expect("job")
        .job
        .unwrap();
    assert!(!saponaio.is_builtin());
    assert_eq!(saponaio.workplace_kind(), CarriageKind::Officina);
    assert!(matches!(w.catalog().job(saponaio).work, Work::Make(_)));
    let stations_after: usize = w.carriages.iter().map(|c| c.stations.len()).sum();
    assert!(
        stations_after > stations_before,
        "no stations for the new job"
    );
    let vital = |w: &World| {
        [Job::Contadino, Job::Cuoco, Job::Mercante]
            .map(|j| w.npcs.iter().filter(|n| n.job == Some(j)).count())
    };
    w.run(&mut brain, 3 * DAY);
    let workers = w.npcs.iter().filter(|n| n.job == Some(saponaio)).count();
    assert!(workers >= 2, "{workers} saponai");
    assert!(
        w.npcs
            .iter()
            .filter(|n| n.job == Some(saponaio))
            .all(|n| w.carriages[n.workplace.unwrap().index()].kind == CarriageKind::Officina)
    );
    w.run(&mut brain, 3 * DAY);
    assert!(w.economy.counters.made(soap) > 1.0, "no sapone made");
    assert!(w.economy.counters.wasted_share(saponaio) < 1.0);
    // The vital jobs stay staffed.
    assert!(vital(&w).iter().all(|&n| n >= 2), "{:?}", vital(&w));
    assert_eq!(w.money_supply(), money);
}

#[test]
fn a_service_job_gets_workers_and_serves() {
    let mut w = World::generate(5, 20, 250);
    let mut brain = UtilityBrain::new(5);
    let now = w.clock;
    let guard = w
        .apply(&job("Guardia", "Mercato", &[], Some(Need::Social)), now)
        .expect("job")
        .job
        .unwrap();
    assert!(matches!(
        w.catalog().job(guard).work,
        Work::Service {
            need: Need::Social,
            ..
        }
    ));
    w.run(&mut brain, 4 * DAY);
    assert!(w.npcs.iter().filter(|n| n.job == Some(guard)).count() >= 2);
    let worked = w.economy.counters.work_minutes.get(guard.index());
    assert!(worked > 0, "the guards never worked");
    // Someone was around to be served for part of the work.
    assert!(w.economy.counters.wasted_share(guard) < 1.0);
}

#[test]
fn meaningless_proposals_are_rejected_with_a_reason() {
    let w = World::generate(3, 10, 60);
    let why = |d: Draft| w.review(&d.proposal).unwrap_err().0;
    let cases: Vec<(Draft, &str)> = vec![
        (
            item("Verdura", "operaio", Category::Raw, 1, &[("metallo", 1)]),
            "già usato",
        ),
        (
            item(
                "Borraccia",
                "alchimista",
                Category::Durable,
                5,
                &[("metallo", 1)],
            ),
            "non esiste",
        ),
        (
            item("Borraccia", "operaio", Category::Durable, 5, &[("oro", 1)]),
            "«oro»",
        ),
        (
            item(
                "Borraccia",
                "mercante",
                Category::Durable,
                5,
                &[("metallo", 1)],
            ),
            "non fabbrica",
        ),
        (
            item(
                "Borraccia",
                "operaio",
                Category::Durable,
                200,
                &[("rottame", 1)],
            ),
            "varrebbe",
        ),
        (item("Pepite", "operaio", Category::Raw, 3, &[]), "Serra"),
        (
            item(
                "Borraccia",
                "operaio",
                Category::Durable,
                5,
                &[("metallo", 11)],
            ),
            "tra 1 e 10",
        ),
        (
            recipe("Metallo dal nulla", "metallo", 1, &[], "operaio"),
            "ingredienti",
        ),
        (
            recipe("Rifondere", "metallo", 1, &[("rottame", 2)], "operaio"),
            "stessi ingredienti",
        ),
        (
            recipe(
                "Attrezzo facile",
                "attrezzo",
                1,
                &[("rottame", 1)],
                "operaio",
            ),
            "troppo facile",
        ),
        (
            recipe(
                "Rottame dal metallo",
                "rottame",
                3,
                &[("metallo", 1)],
                "operaio",
            ),
            "giro senza costo",
        ),
        (
            recipe("Zuppa di metallo", "metallo", 1, &[("verdura", 2)], "cuoco"),
            "non si conserva",
        ),
        (
            job("Custode notturno", "Dormitorio", &[], Some(Need::Energy)),
            "postazione",
        ),
        (job("Vetraio", "Officina", &["razione"], None), "non si fa"),
        (job("Sognatore", "Officina", &[], None), "servizio"),
        (
            job("Macchinista", "Locomotiva", &["metallo"], None),
            "non esiste",
        ),
        (
            draft(Proposal::Event {
                title: "La pioggia".into(),
                description: "Piove dentro.".into(),
                effects: vec![Effect::Stock {
                    carriage_kind: "Dormitorio".into(),
                    item: "metallo".into(),
                    delta: 5,
                }],
            }),
            "non si conserva",
        ),
        (
            draft(Proposal::Event {
                title: "La festa".into(),
                description: "Si balla.".into(),
                effects: vec![Effect::Need {
                    carriage_kind: None,
                    need: Need::Social,
                    delta: 0.9,
                }],
            }),
            "tra -0.3 e 0.3",
        ),
    ];
    for (d, expected) in cases {
        let reason = why(d.clone());
        assert!(
            reason.contains(expected),
            "{}: «{reason}» doesn't say «{expected}»",
            d.proposal.name()
        );
    }
    // A statistic that reads what doesn't exist.
    let stat: Draft = serde_json::from_str(
        r#"{"motivo": "m", "novita": {"tipo": "statistica", "nome": "Sapone pro capite",
        "descrizione": "Quanto sapone c'è.", "scala": [0, 10],
        "formula": {"sorgente": {"scorta": "sapone"}}}}"#,
    )
    .unwrap();
    let reason = why(stat);
    assert!(reason.contains("non esiste"), "{reason}");
}

#[test]
fn a_scheduled_event_applies_once_at_its_minute() {
    let mut w = World::generate(9, 10, 60);
    let mut brain = UtilityBrain::new(9);
    let at = GameTime::from_dhm(1, 9, 0);
    let seq = w.schedule(
        draft(Proposal::Event {
            title: "Il raccolto grande".into(),
            description: "Le serre danno il doppio.".into(),
            effects: vec![
                Effect::Stock {
                    carriage_kind: "Serre".into(),
                    item: "cotone".into(),
                    delta: 20,
                },
                Effect::Need {
                    carriage_kind: None,
                    need: Need::Energy,
                    delta: 0.1,
                },
            ],
        }),
        at,
    );
    let cotone = |w: &World| w.stock_in(CarriageKind::Serra).get(ItemKind::Cotone);
    w.run(&mut brain, at.since(w.clock));
    assert_eq!(w.clock, at);
    assert!(w.decision(seq).is_none() && w.pending().len() == 1);
    let before = cotone(&w);
    let energy: f32 = w.npcs.iter().map(|n| n.needs.energy).sum();
    w.run(&mut brain, 1);
    let d = w.decision(seq).expect("decided");
    assert_eq!(d.at, at);
    assert!(d.result.is_ok(), "{:?}", d.result);
    assert!(w.pending().is_empty());
    // +20 cotone (the Serre had room), once; energy up for everyone (minus
    // a minute of tiredness).
    assert!(cotone(&w) > before + 15.0, "{before} → {}", cotone(&w));
    let after: f32 = w.npcs.iter().map(|n| n.needs.energy).sum();
    assert!(after > energy + 0.05 * w.npcs.len() as f32 * 0.5);
    let applied = cotone(&w);
    w.run(&mut brain, 2 * 60);
    assert!(cotone(&w) < applied + 15.0, "applied twice");
    assert_eq!(w.applied().count(), 1);
}

#[test]
fn replaying_the_applied_list_gives_the_same_world() {
    let mut a = World::generate(21, 12, 120);
    let mut brain = UtilityBrain::new(21);
    let money = a.money_supply();
    a.schedule(
        item(
            "Lanterna",
            "operaio",
            Category::Durable,
            20,
            &[("metallo", 1), ("tessuto", 1)],
        ),
        GameTime::from_dhm(1, 12, 0),
    );
    a.schedule(
        job("Lanternaio", "Officina", &["lanterna"], None),
        GameTime::from_dhm(1, 13, 0),
    );
    a.schedule(
        job("Guardiano", "Mensa", &[], Some(Need::Social)),
        GameTime::from_dhm(2, 8, 0),
    );
    a.schedule(
        recipe(
            "Lanterna ai bulloni",
            "lanterna",
            1,
            &[("metallo", 2)],
            "operaio",
        ),
        GameTime::from_dhm(2, 9, 0),
    );
    a.run(&mut brain, 4 * DAY);
    assert_eq!(a.applied().count(), 4, "{:?}", a.decisions());
    assert_eq!(a.money_supply(), money);

    // A new world with the same seed, the saved list scheduled again.
    let mut b = World::generate(21, 12, 120);
    let mut brain_b = UtilityBrain::new(21);
    for (at, d) in a.replay_list() {
        b.schedule(d, at);
    }
    b.run(&mut brain_b, 4 * DAY);
    assert_eq!(json(&a), json(&b));
}

#[test]
fn a_new_recipe_joins_its_job() {
    let mut w = World::generate(4, 10, 80);
    let now = w.clock;
    let applied = w
        .apply(
            &recipe(
                "Coperta rattoppata",
                "coperta",
                1,
                &[("tessuto", 1), ("rottame", 2)],
                "operaio",
            ),
            now,
        )
        .expect("recipe");
    let r = applied.recipe.unwrap();
    assert_eq!(w.catalog().maker(r), Some(Job::Operaio));
    assert!(w.catalog().job(Job::Operaio).work.recipes().contains(&r));
    // It survives a save: the builtin job keeps it.
    let back: World = serde_json::from_str(&json(&w)).unwrap();
    assert!(back.catalog().job(Job::Operaio).work.recipes().contains(&r));
    let back = postcard_roundtrip(&w);
    assert!(back.catalog().job(Job::Operaio).work.recipes().contains(&r));
}

#[test]
fn a_new_durable_good_reaches_the_mercati_and_is_bought() {
    let mut w = World::generate(13, 20, 250);
    let mut brain = UtilityBrain::new(13);
    let money = w.money_supply();
    let now = w.clock;
    let lanterna = w
        .apply(
            &item(
                "Lanterna",
                "operaio",
                Category::Durable,
                12,
                &[("metallo", 1)],
            ),
            now,
        )
        .expect("applied")
        .item
        .unwrap();
    assert!(w.catalog().item(lanterna).sold);
    assert!(w.catalog().sold_items().contains(&lanterna));
    w.run(&mut brain, 10 * DAY);
    let at_mercati = w.stock_in(CarriageKind::Mercato).get(lanterna);
    let owned: u32 = w
        .npcs
        .iter()
        .map(|n| n.inventory.items.count(lanterna))
        .sum();
    assert!(
        at_mercati >= 1.0 || owned > 0,
        "on the shelves {at_mercati}, owned {owned}"
    );
    let bought = w
        .events
        .iter()
        .filter(|e| matches!(e.kind, sim::EventKind::ItemBought { item, .. } if item == lanterna))
        .count();
    assert!(
        owned > 0 || bought > 0,
        "nobody bought one (on the shelves {at_mercati})"
    );
    assert_eq!(w.money_supply(), money);
    // The builtin tools and clothes still reach the Mercati.
    let tools = w.stock_in(CarriageKind::Mercato).get(ItemKind::Attrezzo);
    let clothes = w.stock_in(CarriageKind::Mercato).get(ItemKind::Vestito);
    assert!(tools + clothes > 5.0, "attrezzi {tools}, vestiti {clothes}");
}
