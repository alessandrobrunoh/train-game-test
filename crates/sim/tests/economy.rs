//! Economia: la moneta si conserva, salari dalla tesoreria, austerità,
//! livello delle paghe, tasse, eredità, acquisti del giocatore, contadini.

use sim::{
    Action, Brain, CarriageId, CarriageKind, EventKind, GameTime, ItemKind, Job, LifeStage,
    MINUTES_PER_DAY, NpcId, SimParams, StationKind, Stats, UtilityBrain, World,
};

const DAY: u64 = MINUTES_PER_DAY;

/// No random deaths, births, thefts nor protests, no savings tax.
fn quiet() -> SimParams {
    SimParams {
        mortality_base: 0.0,
        birth_chance_per_year: 0.0,
        deliberation_rate: 0.0,
        savings_tax_rate: 0.0,
        max_events: usize::MAX,
        ..SimParams::default()
    }
}

fn world(params: SimParams) -> (World, UtilityBrain) {
    (
        World::generate_with_params(42, 10, 100, params),
        UtilityBrain::new(42),
    )
}

/// Runs until just after the next midnight (payday).
fn past_midnight(w: &mut World, brain: &mut dyn Brain) {
    let midnight = w.clock.next_at(0, 0);
    w.run(brain, midnight - w.clock + 1);
}

/// Runs until one minute before the next midnight.
fn before_midnight(w: &mut World, brain: &mut dyn Brain) {
    let midnight = w.clock.next_at(0, 0);
    w.run(brain, midnight - w.clock - 1);
}

fn first_of(w: &World, kind: CarriageKind) -> CarriageId {
    w.carriages.iter().find(|c| c.kind == kind).unwrap().id
}

fn count(w: &World, pred: impl Fn(&EventKind) -> bool) -> usize {
    w.events.iter().filter(|e| pred(&e.kind)).count()
}

fn npc_tokens(w: &World) -> u64 {
    w.npcs.iter().map(|n| u64::from(n.inventory.tokens)).sum()
}

/// Puts a Mercante at the first counter of `market`, so the player can buy.
fn staff_counter(w: &mut World, market: CarriageId) {
    let counter = w.carriages[market.index()]
        .free_station(StationKind::Counter)
        .unwrap();
    let m = w
        .npcs
        .iter()
        .position(|n| n.job == Some(Job::Mercante))
        .unwrap();
    if let Some(s) = w.npcs[m].action.station() {
        let c = w.npcs[m].carriage;
        w.carriages[c.index()].stations[s.index()].occupancy -= 1;
    }
    w.npcs[m].carriage = market;
    w.npcs[m].action = Action::Work(counter);
    w.npcs[m].action_since = w.clock;
    w.npcs[m].action_until = w.clock + 60;
    w.carriages[market.index()].stations[counter.index()].occupancy += 1;
}

#[test]
fn generation_sizes_the_treasury_on_the_payroll() {
    let (w, _) = world(SimParams::default());
    let p = &w.params;
    let workers = w.npcs.iter().filter(|n| n.job.is_some()).count() as f32;
    let jobless = w.npcs.len() as f32 - workers;
    let payroll = workers * 7.0 * p.wage_per_hour as f32 + jobless * p.stipend_per_day as f32;
    let expected = payroll * p.treasury_reserve_days;
    assert!(
        (w.economy.treasury as f32 - expected).abs() <= 1.0,
        "treasury {} for {workers} workers and {jobless} others",
        w.economy.treasury
    );
    assert_eq!(w.economy.pay_level, 1.0);
    assert_eq!(w.money_supply(), w.economy.treasury + npc_tokens(&w));
    for n in &w.npcs {
        let t = n.inventory.tokens;
        if n.job.is_some() {
            assert!((30..120).contains(&t), "worker with {t} tokens");
        } else {
            assert!((5..45).contains(&t), "{} with {t} tokens", n.name);
        }
    }
    // A train twice as big starts with about twice the treasury.
    let big = World::generate(42, 20, 200);
    let ratio = big.economy.treasury as f32 / w.economy.treasury as f32;
    assert!((1.6..2.4).contains(&ratio), "ratio {ratio}");
    let s = Stats::of(&w);
    assert_eq!(s.treasury, w.economy.treasury);
    assert_eq!(s.money_supply, w.money_supply());
    assert!(s.tokens_per_adult > 10.0 && s.median_tokens_per_adult >= 10);
    assert!((0.0..0.6).contains(&s.tokens_gini));
    assert_eq!(s.wage_per_hour, 1.0);
}

/// Every token stays in the sim through wages, stipends, purchases, taxes,
/// fines, help, births, deaths and inheritances; only the player's purchases
/// bring new ones in.
#[test]
fn money_is_conserved_for_30_days() {
    let params = SimParams {
        mortality_base: 2e-3,
        birth_chance_per_year: 2.0,
        theft_temptation_per_hour: 0.5,
        savings_tax_threshold: 20,
        max_events: usize::MAX,
        ..SimParams::default()
    };
    let mut w = World::generate_with_params(7, 10, 150, params);
    let mut brain = UtilityBrain::new(7);
    let market = first_of(&w, CarriageKind::Mercato);
    let serra = first_of(&w, CarriageKind::Serra);
    let mut expected = w.money_supply();
    let mut player_tokens = 1000u32;
    for day in 0..30 {
        for minute in 0..DAY {
            w.run(&mut brain, 1);
            assert_eq!(
                w.money_supply(),
                expected,
                "money changed on day {day} at {}",
                w.clock
            );
            // Now and then the player shops, takes and gives.
            if minute == 11 * 60 && day % 3 == 0 {
                staff_counter(&mut w, market);
                let item = [ItemKind::Vestito, ItemKind::Attrezzo][day % 2];
                if let Ok(price) = w.player_buy(market, item, &mut player_tokens) {
                    expected += u64::from(price);
                }
                w.player_take(serra, ItemKind::Verdura, 2);
                let id = w.npcs[day % w.npcs.len()].id;
                let _ = w.player_give(id, ItemKind::Razione);
                assert_eq!(w.money_supply(), expected, "player on day {day}");
            }
        }
    }
    let c = &w.economy.counters;
    let d = &w.deliberation_counters;
    assert!(
        w.life.births_total > 0 && w.life.deaths_total > 0,
        "{:?}",
        w.life
    );
    assert!(c.pay > 0 && c.purchases > 0 && c.taxes > 0, "{c:?}");
    assert!(c.player_purchases > 0, "the player never bought");
    assert!(d.thefts > 0 && d.help_given > 0, "{d:?}");
    assert!(c.inherited + c.estates > 0, "{c:?}");
    // Nobody held tokens for nothing: the treasury is neither empty nor all.
    assert!(w.economy.treasury > 0 && npc_tokens(&w) > 0);
}

#[test]
fn wages_are_paid_at_midnight_from_the_treasury() {
    let (mut w, mut brain) = world(quiet());
    w.run(&mut brain, 12 * 60);
    before_midnight(&mut w, &mut brain);
    let worker = w
        .npcs
        .iter()
        .find(|n| n.job.is_some() && w.economy.pending_pay(n.id) > 0)
        .expect("someone worked today");
    let (id, tokens, due) = (
        worker.id,
        worker.inventory.tokens,
        w.economy.pending_pay(worker.id),
    );
    let child = w.npcs.iter().find(|n| n.job.is_none()).unwrap();
    let (child, child_tokens) = (child.id, child.inventory.tokens);
    let treasury = w.economy.treasury;
    let pay_before = w.economy.counters.pay;
    w.run(&mut brain, 2);
    assert_eq!(w.npc(id).unwrap().inventory.tokens, tokens + due as u32);
    assert_eq!(w.economy.pending_pay(id), 0);
    let stipend = w.params.stipend_per_day;
    assert_eq!(
        w.npc(child).unwrap().inventory.tokens,
        child_tokens + stipend
    );
    let paid = w.economy.counters.pay - pay_before;
    assert_eq!(w.economy.treasury, treasury - paid);
    assert_eq!(w.economy.counters.pay_withheld, 0);
}

#[test]
fn austerity_pays_everyone_the_same_share() {
    let (mut w, mut brain) = world(quiet());
    w.run(&mut brain, 12 * 60);
    before_midnight(&mut w, &mut brain);
    let workers: Vec<(NpcId, u32, u64)> = w
        .npcs
        .iter()
        .filter(|n| n.job.is_some())
        .map(|n| (n.id, n.inventory.tokens, w.economy.pending_pay(n.id)))
        .collect();
    let jobless = (w.npcs.len() - workers.len()) as u64;
    let due: u64 =
        workers.iter().map(|w| w.2).sum::<u64>() + jobless * u64::from(w.params.stipend_per_day);
    assert!(due > 100);
    // Half, rounded up: with an odd `due` a ratio just below 1/2 would round
    // most shares down by a whole token.
    w.economy.treasury = due.div_ceil(2);
    let supply = npc_tokens(&w) + due.div_ceil(2);
    w.run(&mut brain, 2);
    assert_eq!(w.money_supply(), supply);
    assert!(w.economy.treasury < 50, "{} left", w.economy.treasury);
    assert!(w.economy.counters.pay_withheld >= due / 2 - 1);
    assert_eq!(w.economy.counters.austerity_days, 1);
    for &(id, tokens, owed) in &workers {
        let got = u64::from(w.npc(id).unwrap().inventory.tokens - tokens);
        assert!(
            got <= owed.div_ceil(2) && got + 1 >= owed / 2,
            "{id:?} got {got} of {owed}"
        );
    }
    let logged = |w: &World| count(w, |k| matches!(k, EventKind::Austerity { .. }));
    assert_eq!(logged(&w), 1);
    assert!(matches!(
        w.events
            .iter()
            .find(|e| matches!(e.kind, EventKind::Austerity { .. }))
            .unwrap()
            .kind,
        EventKind::Austerity { paid_percent } if (45..=50).contains(&paid_percent)
    ));
    // Another dry payday the next night: counted, but not logged again so soon.
    w.economy.treasury = 0;
    past_midnight(&mut w, &mut brain);
    assert_eq!(w.economy.counters.austerity_days, 2);
    assert_eq!(logged(&w), 1);
    let event = w
        .events
        .iter()
        .find(|e| matches!(e.kind, EventKind::Austerity { .. }))
        .unwrap();
    assert!(event.to_string().contains("% di salari"), "{event}");
}

#[test]
fn pay_level_follows_the_treasury_within_bounds() {
    let (mut w, mut brain) = world(quiet());
    let market = first_of(&w, CarriageKind::Mercato);
    // Price of an Attrezzo on a full shelf: base value plus transport from
    // the nearest Officina that makes it, times the pay level.
    let full_price = |w: &mut World| {
        let cap = w.params.market_goods_cap;
        w.carriages[market.index()]
            .stock
            .set(ItemKind::Attrezzo, cap);
        w.price(market, ItemKind::Attrezzo).unwrap()
    };
    let (_, distance) = w.nearest_producer(market, ItemKind::Attrezzo).unwrap();
    let transport = (w.params.transport_per_carriage * distance as f32).round() as u32;
    let base = ItemKind::Attrezzo.base_value() + transport;
    assert_eq!(full_price(&mut w), base);
    // A flush treasury: pay (and prices) go up a step a day, up to the cap.
    let extra = 10 * w.economy.treasury;
    w.economy.treasury += extra;
    for day in 1..=5 {
        past_midnight(&mut w, &mut brain);
        let expected = 1.0 + day as f32 * w.params.pay_adjust_per_day;
        assert!((w.economy.pay_level - expected).abs() < 1e-4);
    }
    w.economy.pay_level = 1.995;
    past_midnight(&mut w, &mut brain);
    past_midnight(&mut w, &mut brain);
    assert_eq!(w.economy.pay_level, w.params.pay_level_max);
    assert_eq!(full_price(&mut w), 2 * base);
    assert_eq!(Stats::of(&w).wage_per_hour, 2.0);
    let raised = |w: &World| {
        count(w, |k| {
            matches!(k, EventKind::PayChanged { raised: true, .. })
        })
    };
    assert_eq!(raised(&w), 1);
    // An empty one: down a step a day, down to the floor.
    w.economy.treasury = 0;
    let supply = npc_tokens(&w);
    past_midnight(&mut w, &mut brain);
    assert!(w.economy.pay_level < w.params.pay_level_max);
    w.economy.pay_level = 0.52;
    for _ in 0..5 {
        past_midnight(&mut w, &mut brain);
    }
    assert_eq!(w.economy.pay_level, w.params.pay_level_min);
    assert_eq!(full_price(&mut w), (base as f32 / 2.0).round() as u32);
    // Logged once each way: rate limited, and only for big moves.
    let cut = count(&w, |k| {
        matches!(k, EventKind::PayChanged { raised: false, .. })
    });
    assert_eq!((raised(&w), cut), (1, 1));
    assert_eq!(w.money_supply(), supply);
}

#[test]
fn savings_above_the_threshold_are_taxed() {
    let params = SimParams {
        savings_tax_rate: 0.1,
        ..quiet()
    };
    let (mut w, mut brain) = world(params);
    let i = w
        .npcs
        .iter()
        .position(|n| n.stage() == LifeStage::Bambino)
        .unwrap();
    w.npcs[i].inventory.tokens = 260;
    let id = w.npcs[i].id;
    let supply = w.money_supply();
    past_midnight(&mut w, &mut brain);
    // Stipend first (262), then 10% of the 202 above 60.
    assert_eq!(w.npc(id).unwrap().inventory.tokens, 262 - 20);
    assert!(w.economy.counters.taxes >= 20);
    assert_eq!(w.money_supply(), supply);
}

#[test]
fn estates_without_heirs_go_to_the_treasury() {
    let params = SimParams {
        mortality_base: 1e-12,
        mortality_growth: 0.2,
        ..quiet()
    };
    let (mut w, mut brain) = world(params);
    let i = w
        .npcs
        .iter()
        .position(|n| n.partner().is_none() && n.children().next().is_none() && n.job.is_none())
        .unwrap();
    let id = w.npcs[i].id;
    w.npcs[i].inventory.tokens = 50;
    // Turns 200 at the coming midnight: certain death.
    let midnight = w.clock.next_at(0, 0).0 as i64;
    let year = w.params.minutes_per_year() as i64;
    w.npcs[i].born = midnight - 200 * year - 1;
    w.npcs[i].age = 199;
    let supply = w.money_supply();
    past_midnight(&mut w, &mut brain);
    assert!(w.npc(id).is_none());
    // Paid the stipend at payday, then died: all of it to the treasury.
    assert_eq!(
        w.economy.counters.estates,
        50 + u64::from(w.params.stipend_per_day)
    );
    assert_eq!(w.money_supply(), supply);
}

#[test]
fn player_purchases_feed_the_treasury() {
    let (mut w, _) = world(quiet());
    w.clock = GameTime::from_dhm(1, 10, 0);
    let market = first_of(&w, CarriageKind::Mercato);
    staff_counter(&mut w, market);
    let (treasury, supply) = (w.economy.treasury, w.money_supply());
    let mut tokens = 100;
    let price = w
        .player_buy(market, ItemKind::Vestito, &mut tokens)
        .unwrap();
    assert_eq!(tokens, 100 - price);
    assert_eq!(w.economy.treasury, treasury + u64::from(price));
    assert_eq!(w.money_supply(), supply + u64::from(price));
    assert_eq!(w.economy.counters.player_purchases, u64::from(price));
    // Taking and giving move goods, not money.
    w.player_take(first_of(&w, CarriageKind::Serra), ItemKind::Verdura, 3);
    assert_eq!(w.money_supply(), supply + u64::from(price));
}

#[test]
fn npc_purchases_feed_the_treasury() {
    let (mut w, mut brain) = world(quiet());
    let supply = w.money_supply();
    w.run(&mut brain, 3 * DAY);
    let c = &w.economy.counters;
    assert!(c.purchases > 0, "nobody bought anything");
    let spent: u64 = w
        .events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::ItemBought { price, .. } => Some(u64::from(price)),
            _ => None,
        })
        .sum();
    assert_eq!(c.purchases, spent);
    assert_eq!(w.money_supply(), supply);
}

#[test]
fn farmers_follow_the_serre() {
    let farmers = |w: &World| {
        w.npcs
            .iter()
            .filter(|n| n.job == Some(Job::Contadino))
            .count()
    };
    let fill = |w: &mut World, share: f32| {
        let cap = w.params.verdura_storage_cap;
        for c in w
            .carriages
            .iter_mut()
            .filter(|c| c.kind == CarriageKind::Serra)
        {
            c.stock.set(ItemKind::Verdura, cap * share);
        }
    };
    let (mut w, mut brain) = world(quiet());
    before_midnight(&mut w, &mut brain);
    let start = farmers(&w);
    // Full Serre: fewer farmers (the harvest would rot).
    fill(&mut w, 1.0);
    w.run(&mut brain, 2);
    let full = farmers(&w);
    assert!(full < start, "{start} -> {full} with full Serre");
    // Empty Serre: more farmers again.
    before_midnight(&mut w, &mut brain);
    fill(&mut w, 0.0);
    w.run(&mut brain, 2);
    let empty = farmers(&w);
    assert!(empty > full, "{full} -> {empty} with empty Serre");
    // Tools count: everyone with a tool needs fewer farmers than nobody.
    before_midnight(&mut w, &mut brain);
    let target = w.params.verdura_target_fill;
    fill(&mut w, target);
    for n in w.npcs.iter_mut().filter(|n| n.job == Some(Job::Contadino)) {
        n.inventory.tool = None;
    }
    w.run(&mut brain, 2);
    let without = farmers(&w);
    before_midnight(&mut w, &mut brain);
    let target = w.params.verdura_target_fill;
    fill(&mut w, target);
    for n in w.npcs.iter_mut().filter(|n| n.job == Some(Job::Contadino)) {
        n.inventory.tool = Some(1.0);
    }
    w.run(&mut brain, 2);
    assert!(farmers(&w) < without, "tools ignored: {without}");
}

#[test]
fn economy_survives_save_and_load() {
    let (mut a, mut brain_a) = world(SimParams::default());
    a.run(&mut brain_a, DAY + 17 * 60);
    assert!(a.npcs.iter().any(|n| a.economy.pending_pay(n.id) > 0));
    let mut b: World = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
    let mut brain_b = brain_a.clone();
    a.run(&mut brain_a, DAY);
    b.run(&mut brain_b, DAY);
    assert_eq!(a.economy, b.economy);
    assert_eq!(a.money_supply(), b.money_supply());
}
