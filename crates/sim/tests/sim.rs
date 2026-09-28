use std::collections::HashMap;

use sim::{
    Action, ActionKind, Brain, CarriageId, CarriageKind, DecisionRequest, EventKind, GameTime,
    ItemKind, Job, MINUTES_PER_DAY, NpcId, SimParams, StationId, StationKind, UtilityBrain, World,
};

const DAY: u64 = MINUTES_PER_DAY;

fn world() -> (World, UtilityBrain) {
    (World::generate(42, 10, 100), UtilityBrain::new(42))
}

fn snapshot(world: &World) -> String {
    serde_json::to_string(world).expect("world serializes")
}

/// Station occupancy must match the NPCs actually using each station.
fn assert_occupancy_consistent(world: &World) {
    let mut used: HashMap<(CarriageId, StationId), u16> = HashMap::new();
    for npc in &world.npcs {
        if let Some(s) = npc.action.station() {
            *used.entry((npc.carriage, s)).or_default() += 1;
        }
    }
    for c in &world.carriages {
        for s in &c.stations {
            let expected = used.get(&(c.id, s.id)).copied().unwrap_or(0);
            assert_eq!(
                s.occupancy, expected,
                "occupancy of {} station {:?}",
                c.name, s.id
            );
            assert!(s.occupancy <= s.capacity, "station over capacity");
        }
    }
}

#[test]
fn same_seed_same_state() {
    let (mut a, mut brain_a) = world();
    let (mut b, mut brain_b) = world();
    assert_eq!(snapshot(&a), snapshot(&b));
    a.run(&mut brain_a, 3 * DAY);
    b.run(&mut brain_b, 3 * DAY);
    assert_eq!(snapshot(&a), snapshot(&b));

    let mut c = World::generate(43, 10, 100);
    c.run(&mut UtilityBrain::new(43), 3 * DAY);
    assert_ne!(snapshot(&a), snapshot(&c));
}

#[test]
fn save_load_roundtrip_continues_identically() {
    let (mut a, mut brain_a) = world();
    a.run(&mut brain_a, DAY + 123);
    let mut b: World = serde_json::from_str(&snapshot(&a)).expect("world deserializes");
    let mut brain_b = brain_a.clone();
    a.run(&mut brain_a, DAY);
    b.run(&mut brain_b, DAY);
    assert_eq!(snapshot(&a), snapshot(&b));
}

#[test]
fn needs_stay_in_range_and_stations_consistent() {
    let (mut w, mut brain) = world();
    for t in 0..5 * DAY {
        w.tick(&mut brain);
        for npc in &w.npcs {
            let n = npc.needs;
            for v in [n.hunger, n.energy, n.social] {
                assert!((0.0..=1.0).contains(&v), "need out of range: {v}");
            }
        }
        if t % 97 == 0 {
            assert_occupancy_consistent(&w);
        }
    }
}

#[test]
fn economy_is_sustainable_for_30_days() {
    let (mut w, mut brain) = world();
    // Counts events over the whole run: keep them all.
    w.params.max_events = usize::MAX;
    w.run(&mut brain, 30 * DAY);
    let deaths = w
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NpcDied { .. }))
        .count();
    assert_eq!(deaths, 0);
    assert_eq!(w.npcs.len(), 100);
    let razioni = w.available(ItemKind::Razione);
    assert!(razioni > 50.0, "mense almost empty: {razioni}");
    let stats = sim::Stats::of(&w);
    assert!(
        stats.avg_needs.hunger > 0.4,
        "population is hungry: {stats}"
    );
}

#[test]
fn daily_rhythm_sleep_at_night_work_by_day() {
    let (mut w, mut brain) = world();
    w.run(&mut brain, GameTime::from_dhm(3, 3, 0) - w.clock);
    let night = sim::Stats::of(&w);
    assert!(night.count(sim::ActionKind::Sleep) >= 90, "night: {night}");
    w.run(&mut brain, GameTime::from_dhm(3, 10, 30) - w.clock);
    let day = sim::Stats::of(&w);
    assert!(day.count(sim::ActionKind::Work) >= 50, "day: {day}");
    assert!(day.count(sim::ActionKind::Sleep) <= 5, "day: {day}");
}

#[test]
fn travel_moves_npc_to_destination() {
    let (mut w, mut brain) = world();
    let id = w.npcs[0].id;
    let from = w.npcs[0].carriage;
    let to = CarriageId(if from.0 < 5 { from.0 + 5 } else { from.0 - 5 });
    let minutes = w.travel_minutes(from, to);
    let now = w.clock;
    let npc = &mut w.npcs[0];
    npc.action = Action::Travel { to };
    npc.action_since = now;
    npc.action_until = now + minutes;

    w.run(&mut brain, minutes);
    assert_eq!(
        w.npc(id).unwrap().carriage,
        from,
        "arrives only when the trip ends"
    );
    w.tick(&mut brain);
    assert_eq!(w.npc(id).unwrap().carriage, to);

    // And during normal play people actually move around the train.
    let before: Vec<_> = w.npcs.iter().map(|n| n.carriage).collect();
    w.run(&mut brain, DAY / 2);
    let moved = w
        .npcs
        .iter()
        .zip(&before)
        .filter(|(n, c)| n.carriage != **c)
        .count();
    assert!(moved > 10, "only {moved} NPCs moved");
}

#[test]
fn generated_options_are_valid() {
    let (mut w, mut brain) = world();
    for _ in 0..12 {
        w.run(&mut brain, 173);
        let ids: Vec<_> = w.npcs.iter().map(|n| n.id).collect();
        for id in ids {
            let options = w.options(id);
            let npc = w.npc(id).unwrap();
            let here = &w.carriages[npc.carriage.index()];
            assert_eq!(options.first().map(|o| o.action), Some(Action::Idle));
            for o in &options {
                assert!(!o.description.is_empty());
                assert!(o.minutes > 0);
                let station = |s: StationId| here.station(s).expect("station exists");
                match o.action {
                    Action::Eat(s) => {
                        assert_eq!(here.kind, CarriageKind::Mensa);
                        assert_eq!(station(s).kind, StationKind::Table);
                        assert!(station(s).has_room());
                        assert!(here.stock.get(ItemKind::Razione) >= w.params.razioni_per_meal);
                    }
                    Action::Sleep(s) => {
                        assert_eq!(station(s).kind, StationKind::Bed);
                        assert!(station(s).has_room());
                    }
                    Action::Work(s) => {
                        let job = npc.job.expect("only workers work");
                        assert!(job.in_shift(w.clock));
                        assert_eq!(npc.workplace, Some(here.id));
                        assert_eq!(station(s).kind, job.station_kind());
                        assert!(station(s).has_room());
                    }
                    Action::Travel { to } => {
                        assert_ne!(to, here.id);
                        assert!(w.carriage(to).is_some());
                        assert!(o.goal.is_some());
                        assert_eq!(o.minutes, w.travel_minutes(here.id, to).max(1));
                    }
                    Action::Socialize(other) => {
                        assert_ne!(other, id);
                        assert_eq!(w.npc(other).unwrap().carriage, here.id);
                    }
                    Action::Buy(item) => {
                        assert_eq!(here.kind, CarriageKind::Mercato);
                        assert!(npc.wants(item));
                        assert!(here.stock.count(item) >= 1);
                        assert!(w.price(here.id, item).unwrap() <= npc.inventory.tokens);
                    }
                    Action::Idle => {}
                }
            }
        }
    }
}

/// Picks nonsense indices: the sim must cope (NPCs idle briefly).
struct BrokenBrain;

impl Brain for BrokenBrain {
    fn decide(&mut self, _: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        vec![usize::MAX; requests.len() / 2]
    }
}

#[test]
fn invalid_choices_fall_back_to_idle() {
    let (mut w, _) = world();
    w.run(&mut BrokenBrain, 60);
    assert!(w.npcs.iter().all(|n| n.action == Action::Idle));
}

/// Checks the batching contract: one call per tick, each NPC at most once.
struct BatchCheck {
    inner: UtilityBrain,
    calls: u64,
    last: Option<GameTime>,
}

impl Brain for BatchCheck {
    fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
        assert_ne!(
            self.last,
            Some(world.clock),
            "decide called twice in one tick"
        );
        self.last = Some(world.clock);
        self.calls += 1;
        let mut seen = std::collections::HashSet::new();
        for r in requests {
            assert!(seen.insert(r.npc));
            assert!(!r.options.is_empty());
            assert!(r.options.iter().all(|o| !o.description.is_empty()));
        }
        self.inner.decide(world, requests)
    }
}

#[test]
fn decisions_are_batched_per_tick() {
    let (mut w, inner) = world();
    let mut brain = BatchCheck {
        inner,
        calls: 0,
        last: None,
    };
    w.run(&mut brain, DAY);
    assert!(brain.calls > 100);
}

#[test]
fn starvation_kills_and_is_logged() {
    let params = SimParams {
        verdura_per_farm_minute: 0.0,
        starvation_minutes: 12 * 60,
        ..SimParams::default()
    };
    let mut w = World::generate_with_params(1, 8, 30, params);
    for c in &mut w.carriages {
        c.stock.set(ItemKind::Verdura, 0.0);
        c.stock.set(ItemKind::Razione, 0.0);
    }
    w.run(&mut UtilityBrain::new(1), 3 * DAY);
    assert!(
        w.npcs.is_empty(),
        "{} NPCs survived without food",
        w.npcs.len()
    );
    let kinds: Vec<_> = w.events.iter().map(|e| &e.kind).collect();
    assert!(kinds.iter().any(|k| matches!(
        k,
        EventKind::Shortage {
            item: ItemKind::Razione
        }
    )));
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EventKind::NpcStarving { .. }))
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|k| matches!(k, EventKind::NpcDied { .. }))
            .count(),
        30
    );
    assert_occupancy_consistent(&w);
}

#[test]
fn npc_context_describes_state() {
    let (w, _) = world();
    let npc = &w.npcs[0];
    let text = w.npc_context(npc.id).unwrap();
    assert!(text.contains(&npc.name));
    assert!(text.contains("Giorno 1 06:00"));
    assert!(text.contains("carrozza"));
    assert!(text.contains("gettoni"));
}

#[test]
fn default_layout_has_markets_and_merchants() {
    let w = World::generate(42, 20, 400);
    let count = |kind| w.carriages.iter().filter(|c| c.kind == kind).count();
    assert_eq!(count(CarriageKind::Mercato), 2);
    for kind in CarriageKind::ALL {
        assert!(count(kind) >= 2, "{kind}: {}", count(kind));
    }
    for job in Job::ALL {
        let staff = w.npcs.iter().filter(|n| n.job == Some(job)).count();
        assert!(staff >= 2, "{job}: {staff}");
    }
    for c in w
        .carriages
        .iter()
        .filter(|c| c.kind == CarriageKind::Mercato)
    {
        assert!(c.stations.iter().any(|s| s.kind == StationKind::Counter));
        assert!(w.npcs.iter().any(|n| n.workplace == Some(c.id)));
    }
}

#[test]
fn cooks_turn_verdura_into_razioni() {
    let params = SimParams {
        verdura_per_farm_minute: 0.0,
        ..SimParams::default()
    };
    let mut w = World::generate_with_params(7, 10, 100, params);
    for c in &mut w.carriages {
        c.stock.set(ItemKind::Razione, 0.0);
    }
    let verdura_before = w.total_stock().get(ItemKind::Verdura);
    assert!(verdura_before > 100.0);
    // Until lunch: cooks work from 06:00, few meals happen before 06:00+.
    w.run(
        &mut UtilityBrain::new(7),
        GameTime::from_dhm(1, 12, 0) - w.clock,
    );
    let cooked = w.available(ItemKind::Razione);
    let used = verdura_before - w.total_stock().get(ItemKind::Verdura);
    assert!(used > 20.0, "cooks used only {used} verdura");
    // Everything cooked is either still in the Mense or was eaten.
    assert!(cooked <= used * w.params.razioni_per_verdura + 1e-3);

    // Without Verdura cooking produces nothing.
    for c in &mut w.carriages {
        c.stock.set(ItemKind::Verdura, 0.0);
        c.stock.set(ItemKind::Razione, 0.0);
    }
    w.run(&mut UtilityBrain::new(7), 4 * 60);
    assert_eq!(w.available(ItemKind::Razione), 0.0);
}

#[test]
fn tools_wear_out_and_are_bought_again() {
    let (mut w, mut brain) = world();
    w.params.max_events = usize::MAX;
    w.run(&mut brain, 12 * DAY);
    let mut broke: HashMap<NpcId, GameTime> = HashMap::new();
    let mut rebought = 0;
    for e in &w.events {
        match e.kind {
            EventKind::ItemBroke {
                npc,
                item: ItemKind::Attrezzo,
                ..
            } => {
                broke.insert(npc, e.time);
            }
            EventKind::ItemBought {
                npc,
                item: ItemKind::Attrezzo,
                price,
                carriage,
                ..
            } => {
                assert!(price >= ItemKind::Attrezzo.base_value());
                assert_eq!(w.carriages[carriage.index()].kind, CarriageKind::Mercato);
                if broke.contains_key(&npc) {
                    rebought += 1;
                }
            }
            _ => {}
        }
    }
    assert!(broke.len() >= 10, "only {} tools broke", broke.len());
    assert!(rebought >= 5, "only {rebought} broken tools replaced");
    let with_tool = w
        .npcs
        .iter()
        .filter(|n| n.job.is_some_and(Job::uses_tool) && n.inventory.tool.is_some())
        .count();
    let tool_users = w
        .npcs
        .iter()
        .filter(|n| n.job.is_some_and(Job::uses_tool))
        .count();
    assert!(
        with_tool * 2 > tool_users,
        "{with_tool}/{tool_users} have a tool"
    );
}

#[test]
fn buy_offered_only_when_affordable_and_in_stock() {
    let (mut w, _) = world();
    // Midday, a worker who uses tools, standing in a Mercato, without a tool.
    w.clock = GameTime::from_dhm(1, 10, 0);
    let market = w
        .carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Mercato)
        .unwrap()
        .id;
    let i = w
        .npcs
        .iter()
        .position(|n| n.job.is_some_and(Job::uses_tool))
        .unwrap();
    let id = w.npcs[i].id;
    w.npcs[i].carriage = market;
    w.npcs[i].inventory.tool = None;
    w.npcs[i].inventory.clothes = Some(1.0);
    let buys = |w: &mut World| -> Vec<ItemKind> {
        w.options(id)
            .iter()
            .filter_map(|o| match o.action {
                Action::Buy(item) => Some(item),
                _ => None,
            })
            .collect()
    };
    let price = w.price(market, ItemKind::Attrezzo).unwrap();

    w.npcs[i].inventory.tokens = price - 1;
    assert!(buys(&mut w).is_empty(), "offered without enough tokens");

    w.npcs[i].inventory.tokens = price;
    assert_eq!(buys(&mut w), vec![ItemKind::Attrezzo], "clothes not wanted");

    w.carriages[market.index()]
        .stock
        .set(ItemKind::Attrezzo, 0.5);
    assert!(buys(&mut w).is_empty(), "offered while out of stock");
    w.carriages[market.index()]
        .stock
        .set(ItemKind::Attrezzo, 10.0);

    w.npcs[i].inventory.tool = Some(0.5);
    assert!(buys(&mut w).is_empty(), "offered a second tool");

    // Buying pays the price and hands over the item.
    w.npcs[i].inventory.tool = None;
    let price = w.price(market, ItemKind::Attrezzo).unwrap();
    w.npcs[i].inventory.tokens = price + 3;
    w.npcs[i].action_until = w.clock;
    struct BuyTool;
    impl Brain for BuyTool {
        fn decide(&mut self, _: &World, requests: &[DecisionRequest]) -> Vec<usize> {
            requests
                .iter()
                .map(|r| {
                    r.options
                        .iter()
                        .position(|o| o.action == Action::Buy(ItemKind::Attrezzo))
                        .unwrap_or(0)
                })
                .collect()
        }
    }
    let before = w.carriages[market.index()].stock.get(ItemKind::Attrezzo);
    w.tick(&mut BuyTool);
    let npc = w.npc(id).unwrap();
    assert_eq!(npc.action, Action::Buy(ItemKind::Attrezzo));
    assert_eq!(npc.inventory.tokens, 3);
    assert_eq!(npc.inventory.tool, Some(1.0));
    let after = w.carriages[market.index()].stock.get(ItemKind::Attrezzo);
    assert!((before - after - 1.0).abs() < 1e-4);

    // Markets are closed at night.
    w.npcs[i].inventory.tool = None;
    w.clock = GameTime::from_dhm(2, 23, 0);
    assert!(buys(&mut w).is_empty(), "market open at night");
}

#[test]
fn storage_stays_bounded_for_30_days() {
    let (mut w, mut brain) = world();
    for _ in 0..30 * 24 {
        w.run(&mut brain, 60);
        for c in &w.carriages {
            for (item, amount) in c.stock.iter() {
                let cap = w.params.storage_cap(c.kind, item);
                assert!(
                    (0.0..=cap + 1e-3).contains(&amount),
                    "{} holds {amount} {} (cap {cap}) at {}",
                    c.name,
                    item.plural(),
                    w.clock
                );
            }
        }
        for n in &w.npcs {
            for d in [n.inventory.tool, n.inventory.clothes]
                .into_iter()
                .flatten()
            {
                assert!(d > 0.0 && d <= 1.0);
            }
        }
    }
}

#[test]
fn full_train_lives_30_days_and_economy_circulates() {
    let mut w = World::generate(42, 20, 400);
    w.params.max_events = usize::MAX;
    let mut brain = UtilityBrain::new(42);
    let tokens_start: u64 = w.npcs.iter().map(|n| u64::from(n.inventory.tokens)).sum();
    let mut market_stocked_hours = 0;
    let hours = 30 * 24;
    for _ in 0..hours {
        w.run(&mut brain, 60);
        if SOLD.iter().all(|&i| w.available(i) >= 1.0) {
            market_stocked_hours += 1;
        }
    }
    let deaths = w
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::NpcDied { .. }))
        .count();
    assert_eq!(deaths, 0);
    assert_eq!(w.npcs.len(), 400);
    let count =
        |pred: &dyn Fn(&EventKind) -> bool| w.events.iter().filter(|e| pred(&e.kind)).count();
    let bought = count(&|k| matches!(k, EventKind::ItemBought { .. }));
    let broke = count(&|k| matches!(k, EventKind::ItemBroke { .. }));
    assert!(bought > 30 * 20, "only {bought} purchases");
    assert!(broke > 30 * 20, "only {broke} items broke");
    assert!(
        market_stocked_hours * 10 > hours * 7,
        "markets stocked only {market_stocked_hours}/{hours} hours"
    );
    let stats = sim::Stats::of(&w);
    assert!(
        stats.avg_needs.hunger > 0.4,
        "population is hungry: {stats}"
    );
    assert!(stats.tokens > 0 && stats.tokens != tokens_start);
    assert!(stats.count(ActionKind::Idle) < 400);
}

const SOLD: [ItemKind; 2] = [ItemKind::Attrezzo, ItemKind::Vestito];

#[test]
fn event_log_is_capped_without_changing_the_simulation() {
    let (mut full, mut brain_full) = world();
    full.params.max_events = usize::MAX;
    let (mut capped, mut brain_capped) = world();
    capped.params.max_events = 20;
    for _ in 0..6 * 24 {
        full.run(&mut brain_full, 60);
        capped.run(&mut brain_capped, 60);
        assert!(capped.events.len() <= 20);
        assert_eq!(capped.events_total(), full.events_total());
    }
    assert!(
        full.events.len() > 60,
        "too few events: {}",
        full.events.len()
    );
    // Same world, only the oldest events are gone.
    assert_eq!(capped.npcs, full.npcs);
    assert_eq!(capped.carriages, full.carriages);
    let kept = capped.events.len();
    assert!(kept >= 15);
    assert_eq!(capped.events[..], full.events[full.events.len() - kept..]);
}

fn first_of(w: &World, kind: CarriageKind) -> CarriageId {
    w.carriages.iter().find(|c| c.kind == kind).unwrap().id
}

#[test]
fn player_takes_whole_units_from_storage() {
    let (mut w, _) = world();
    let mensa = first_of(&w, CarriageKind::Mensa);
    w.carriages[mensa.index()].stock.set(ItemKind::Razione, 2.5);
    assert_eq!(w.player_take(mensa, ItemKind::Razione, 1), 1);
    assert_eq!(w.player_take(mensa, ItemKind::Razione, 5), 1);
    assert!((w.carriages[mensa.index()].stock.get(ItemKind::Razione) - 0.5).abs() < 1e-5);
    // Only half a ration left, and no scrap in a Mensa.
    assert_eq!(w.player_take(mensa, ItemKind::Razione, 1), 0);
    assert_eq!(w.player_take(mensa, ItemKind::Rottame, 1), 0);
    assert_eq!(w.player_take(CarriageId(999), ItemKind::Razione, 1), 0);
    let took: Vec<u32> = w
        .events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::PlayerTook {
                item: ItemKind::Razione,
                amount,
                carriage,
            } if carriage == mensa => Some(amount),
            _ => None,
        })
        .collect();
    assert_eq!(took, vec![1, 1]);
    assert!(w.events[0].to_string().contains("Hai preso una razione"));
}

#[test]
fn player_buys_only_from_a_staffed_market() {
    let (mut w, _) = world();
    w.clock = GameTime::from_dhm(1, 10, 0);
    let market = first_of(&w, CarriageKind::Mercato);
    let mensa = first_of(&w, CarriageKind::Mensa);
    let item = ItemKind::Vestito;
    let mut tokens = 100;

    assert_eq!(
        w.player_buy(mensa, ItemKind::Razione, &mut tokens),
        Err(sim::BuyError::NotForSale)
    );
    assert_eq!(
        w.player_buy(market, ItemKind::Razione, &mut tokens),
        Err(sim::BuyError::NotForSale)
    );
    // Nobody is at the counter yet (everyone starts idle).
    assert!(w.merchant_on_duty(market).is_none());
    assert_eq!(
        w.player_buy(market, item, &mut tokens),
        Err(sim::BuyError::NoMerchant)
    );

    // Put a Mercante to work at the first counter.
    let counter = w.carriages[market.index()]
        .free_station(StationKind::Counter)
        .unwrap();
    let m = w
        .npcs
        .iter()
        .position(|n| n.job == Some(Job::Mercante))
        .unwrap();
    w.npcs[m].carriage = market;
    w.npcs[m].action = Action::Work(counter);
    w.carriages[market.index()].stations[counter.index()].occupancy += 1;
    assert_eq!(w.merchant_on_duty(market).map(|n| n.id), Some(w.npcs[m].id));

    let price = w.price(market, item).unwrap();
    let mut poor = price - 1;
    assert_eq!(
        w.player_buy(market, item, &mut poor),
        Err(sim::BuyError::TooExpensive(price))
    );
    assert_eq!(poor, price - 1);

    let before = w.carriages[market.index()].stock.get(item);
    assert_eq!(w.player_buy(market, item, &mut tokens), Ok(price));
    assert_eq!(tokens, 100 - price);
    let after = w.carriages[market.index()].stock.get(item);
    assert!((before - after - 1.0).abs() < 1e-5);
    assert!(matches!(
        w.events.last().unwrap().kind,
        EventKind::PlayerBought { item: ItemKind::Vestito, price: p, carriage } if p == price && carriage == market
    ));

    w.carriages[market.index()].stock.set(item, 0.5);
    assert_eq!(
        w.player_buy(market, item, &mut tokens),
        Err(sim::BuyError::OutOfStock)
    );
}

#[test]
fn player_gives_food_and_goods() {
    let (mut w, _) = world();
    let id = w.npcs[0].id;
    w.npcs[0].needs.hunger = 0.1;
    w.npcs[0].starving_minutes = 5;
    assert_eq!(w.player_give(id, ItemKind::Razione), Ok(()));
    let npc = w.npc(id).unwrap();
    assert!((npc.needs.hunger - (0.1 + w.params.meal_restore)).abs() < 1e-5);
    assert_eq!(npc.starving_minutes, 0);
    assert!(matches!(
        &w.events.last().unwrap().kind,
        EventKind::PlayerGave { npc, item: ItemKind::Razione, .. } if *npc == id
    ));

    // Full: refuses food.
    w.npcs[0].needs.hunger = 1.0;
    assert_eq!(
        w.player_give(id, ItemKind::Verdura),
        Err(sim::GiveError::NotWanted)
    );
    assert_eq!(
        w.player_give(id, ItemKind::Rottame),
        Err(sim::GiveError::NotWanted)
    );

    // Clothes only to who has none, and they arrive new.
    w.npcs[0].inventory.clothes = Some(0.5);
    assert_eq!(
        w.player_give(id, ItemKind::Vestito),
        Err(sim::GiveError::NotWanted)
    );
    w.npcs[0].inventory.clothes = None;
    assert_eq!(w.player_give(id, ItemKind::Vestito), Ok(()));
    assert_eq!(w.npc(id).unwrap().inventory.clothes, Some(1.0));

    assert_eq!(
        w.player_give(NpcId(99_999), ItemKind::Razione),
        Err(sim::GiveError::NoSuchNpc)
    );
    let gifts = w
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::PlayerGave { .. }))
        .count();
    assert_eq!(gifts, 2);
}
