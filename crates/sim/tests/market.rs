//! Mercato: prezzi per distanza dal produttore, specialità delle carrozze,
//! storico dei prezzi, vendite del giocatore.

use sim::{
    Action, CarriageId, CarriageKind, EventKind, GameTime, ItemKind, Job, MINUTES_PER_DAY,
    SellError, SimParams, StationKind, Trend, UtilityBrain, World,
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

fn of_kind(w: &World, kind: CarriageKind) -> Vec<CarriageId> {
    w.carriages
        .iter()
        .filter(|c| c.kind == kind)
        .map(|c| c.id)
        .collect()
}

/// Puts a Mercante at the first free counter of `market`.
fn staff_counter(w: &mut World, market: CarriageId) {
    let counter = w.carriages[market.index()]
        .free_station(StationKind::Counter)
        .unwrap();
    let m = w
        .npcs
        .iter()
        .position(|n| n.job == Some(Job::Mercante) && !matches!(n.action, Action::Work(_)))
        .or_else(|| w.npcs.iter().position(|n| n.job == Some(Job::Mercante)))
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
fn at_equal_stock_price_grows_with_distance() {
    let mut w = World::generate_with_params(3, 40, 300, quiet());
    let market = of_kind(&w, CarriageKind::Mercato)[0];
    let mut officine = of_kind(&w, CarriageKind::Officina);
    officine.sort_by_key(|o| (o.distance(market), o.0));
    let item = ItemKind::Vestito;
    for fill in [0.0, 0.5, 1.0] {
        let cap = w.params.market_goods_cap;
        w.carriages[market.index()].stock.set(item, cap * fill);
        // Each Officina in turn, nearest first, is the only one making Vestiti.
        let mut last: Option<(u32, u32)> = None;
        for &maker in &officine {
            for &o in &officine {
                let items: &[ItemKind] = if o == maker {
                    &[ItemKind::Vestito]
                } else {
                    &[ItemKind::Attrezzo]
                };
                w.set_specialties(o, items);
            }
            let (producer, distance) = w.nearest_producer(market, item).unwrap();
            assert_eq!((producer, distance), (maker, maker.distance(market)));
            let price = w.price(market, item).unwrap();
            if let Some((d, p)) = last {
                assert!(price >= p, "{price} at {distance} < {p} at {d}");
                if distance > d {
                    assert!(price > p, "{price} at {distance}, {p} at {d}");
                }
            }
            last = Some((distance, price));
        }
        let quote = w
            .market_quotes(market)
            .into_iter()
            .find(|q| q.item == item)
            .unwrap();
        assert_eq!(quote.price, w.price(market, item).unwrap());
        assert_eq!(quote.producer, Some(*officine.last().unwrap()));
    }
    // The Mercati of a default train: same stock, different distances.
    let w = World::generate(42, 20, 400);
    for item in ItemKind::SOLD {
        let mut quotes: Vec<(u32, u32, f32)> = of_kind(&w, CarriageKind::Mercato)
            .into_iter()
            .map(|m| {
                let (_, d) = w.nearest_producer(m, item).unwrap();
                (
                    d,
                    w.price(m, item).unwrap(),
                    w.carriages[m.index()].stock.get(item),
                )
            })
            .collect();
        quotes.sort_by_key(|q| q.0);
        for pair in quotes.windows(2) {
            if pair[0].2 == pair[1].2 {
                assert!(pair[1].1 >= pair[0].1, "{item:?} {quotes:?}");
            }
        }
    }
}

#[test]
fn specialties_are_deterministic_and_cover_every_recipe() {
    let mut differ = false;
    for seed in 0..12u64 {
        for n in [1, 3, 4, 5, 10, 20, 33, 40] {
            let a = World::generate(seed, n, 20);
            let b = World::generate(seed, n, 20);
            for c in &a.carriages {
                let s = a.specialties(c.id);
                assert_eq!(s, b.specialties(c.id), "seed {seed} carriage {}", c.id);
                let recipes: Vec<ItemKind> = c.kind.recipes().iter().map(|r| r.output).collect();
                assert!(
                    s.iter().all(|i| recipes.contains(i)),
                    "{s:?} in a {}",
                    c.kind
                );
                assert!(s.windows(2).all(|w| w[0] < w[1]), "{s:?} not sorted");
                match recipes.len() {
                    0 => assert!(s.is_empty()),
                    1 => assert_eq!(s, recipes.as_slice()),
                    _ => assert!(!s.is_empty(), "{} has no specialty", c.label()),
                }
            }
            // Every item made on this train has a specialist, and so a
            // nearest producer from everywhere.
            for item in ItemKind::ALL {
                let made = a.carriages.iter().any(|c| c.kind.makes(item));
                let specialist = a
                    .carriages
                    .iter()
                    .any(|c| a.specialties(c.id).contains(&item));
                assert_eq!(made, specialist, "seed {seed}, {n} carriages: {item:?}");
                for c in &a.carriages {
                    let producer = a.nearest_producer(c.id, item);
                    assert_eq!(producer.is_some(), made);
                    if let Some((p, d)) = producer {
                        assert!(a.specialties(p).contains(&item));
                        assert_eq!(d, p.distance(c.id));
                    }
                }
            }
            if n == 20 {
                let first = World::generate(0, 20, 20);
                differ |= a
                    .carriages
                    .iter()
                    .any(|c| a.specialties(c.id) != first.specialties(c.id));
            }
        }
    }
    assert!(differ, "every seed gives the same specialties");
    // Every Officina specializes: none makes everything.
    let w = World::generate(42, 20, 400);
    let officine = of_kind(&w, CarriageKind::Officina);
    let recipes = CarriageKind::Officina.recipes().len();
    assert!(
        officine
            .iter()
            .all(|&o| (1..recipes).contains(&w.specialties(o).len()))
    );
}

#[test]
fn specialties_do_not_disturb_the_rest_of_the_generation() {
    let with = |chance: f32| {
        let params = SimParams {
            second_specialty_chance: chance,
            ..SimParams::default()
        };
        World::generate_with_params(11, 20, 300, params)
    };
    let (a, b) = (with(0.0), with(1.0));
    let officine = of_kind(&a, CarriageKind::Officina);
    // A second specialty for everyone (the coverage fix-up may add more).
    assert!(officine.iter().all(|&o| b.specialties(o).len() >= 2));
    let total = |w: &World| -> usize { officine.iter().map(|&o| w.specialties(o).len()).sum() };
    assert!(total(&a) < total(&b));
    let json = |v: &World| {
        (
            serde_json::to_string(&v.npcs).unwrap(),
            serde_json::to_string(&v.carriages).unwrap(),
        )
    };
    assert_eq!(json(&a), json(&b));
    assert_eq!(a.economy, b.economy);
}

#[test]
fn officine_make_their_specialties_first() {
    let mut w = World::generate_with_params(5, 20, 400, quiet());
    let officine = of_kind(&w, CarriageKind::Officina);
    let (a, v) = (officine[0], officine[1]);
    w.set_specialties(a, &[ItemKind::Attrezzo]);
    w.set_specialties(v, &[ItemKind::Vestito]);
    // Plenty of inputs (Metallo for tools, Tessuto for clothes), no goods yet.
    for &o in &[a, v] {
        let stock = &mut w.carriages[o.index()].stock;
        stock.set(ItemKind::Attrezzo, 0.0);
        stock.set(ItemKind::Vestito, 0.0);
        stock.set(ItemKind::Metallo, 100.0);
        stock.set(ItemKind::Tessuto, 100.0);
    }
    // The Mercati are full: the Mercanti take nothing from the Officine.
    for m in of_kind(&w, CarriageKind::Mercato) {
        for item in ItemKind::SOLD {
            let cap = w.params.market_goods_cap;
            w.carriages[m.index()].stock.set(item, cap);
        }
    }
    let mut brain = UtilityBrain::new(5);
    w.run(&mut brain, 7 * 60); // until 13:00
    let made = |w: &World, o: CarriageId, item| w.carriages[o.index()].stock.get(item);
    assert!(
        made(&w, a, ItemKind::Attrezzo) > 1.0 && made(&w, a, ItemKind::Vestito) < 1.0,
        "attrezzi {} vestiti {}",
        made(&w, a, ItemKind::Attrezzo),
        made(&w, a, ItemKind::Vestito)
    );
    assert!(
        made(&w, v, ItemKind::Vestito) > 1.0 && made(&w, v, ItemKind::Attrezzo) < 1.0,
        "attrezzi {} vestiti {}",
        made(&w, v, ItemKind::Attrezzo),
        made(&w, v, ItemKind::Vestito)
    );
    // With its specialty's storage full, an Officina makes the rest.
    let cap = w
        .params
        .storage_cap(CarriageKind::Officina, ItemKind::Attrezzo);
    w.carriages[a.index()].stock.set(ItemKind::Attrezzo, cap);
    w.carriages[a.index()].stock.set(ItemKind::Rottame, 100.0);
    // Something other than its specialty gets made (inputs like Metallo and
    // Tessuto are consumed by other recipes, so compare item by item).
    let others: Vec<ItemKind> = CarriageKind::Officina
        .recipes()
        .iter()
        .map(|r| r.output)
        .filter(|&item| item != ItemKind::Attrezzo)
        .collect();
    let before: Vec<f32> = others.iter().map(|&i| made(&w, a, i)).collect();
    w.run(&mut brain, 3 * 60);
    assert!(
        others
            .iter()
            .zip(&before)
            .any(|(&i, &b)| made(&w, a, i) > b + 1.0),
        "nothing else made: {others:?} {before:?}"
    );
}

#[test]
fn prices_are_sampled_daily_with_a_trend() {
    let params = SimParams {
        price_history_days: 5,
        ..quiet()
    };
    let mut w = World::generate_with_params(42, 10, 100, params);
    let market = of_kind(&w, CarriageKind::Mercato)[0];
    assert_eq!(w.price_history().len(), 1);
    assert_eq!(w.price_history()[0].day, 1);
    let series = w.price_series(market, ItemKind::Vestito);
    assert_eq!(series, [(1, w.price(market, ItemKind::Vestito).unwrap())]);
    // No history before today: no trend.
    assert!(
        w.market_quotes(market)
            .iter()
            .all(|q| q.trend == Trend::Flat)
    );
    let mut brain = UtilityBrain::new(42);
    w.run(&mut brain, 8 * DAY);
    let days: Vec<u64> = w.price_history().iter().map(|s| s.day).collect();
    assert_eq!(days, [5, 6, 7, 8, 9]);
    for s in w.price_history() {
        assert!(s.price(0, ItemKind::Attrezzo).is_some());
        assert!(s.price(0, ItemKind::Verdura).is_none());
    }
    // An empty shelf: the price jumps up from the recent mean.
    w.carriages[market.index()]
        .stock
        .set(ItemKind::Attrezzo, 0.0);
    let quote = w
        .market_quotes(market)
        .into_iter()
        .find(|q| q.item == ItemKind::Attrezzo)
        .unwrap();
    assert_eq!(quote.stock, 0);
    assert!(quote.reference.is_some());
    assert_eq!(quote.trend, Trend::Up, "{quote:?}");
    // A full one: down.
    let cap = w.params.market_goods_cap;
    w.carriages[market.index()]
        .stock
        .set(ItemKind::Attrezzo, cap);
    w.carriages[market.index()]
        .stock
        .set(ItemKind::Vestito, cap);
    let history_full = w
        .price_series(market, ItemKind::Vestito)
        .iter()
        .all(|&(_, p)| p <= w.price(market, ItemKind::Vestito).unwrap());
    let quote = w
        .market_quotes(market)
        .into_iter()
        .find(|q| q.item == ItemKind::Vestito)
        .unwrap();
    assert!(history_full || quote.trend == Trend::Down, "{quote:?}");
    // Only Mercati have quotes.
    let serra = of_kind(&w, CarriageKind::Serra)[0];
    assert!(w.market_quotes(serra).is_empty());
    assert_eq!(w.nearest_market(serra), Some(market));
}

#[test]
fn player_sales_are_paid_from_the_treasury() {
    let mut w = World::generate_with_params(42, 10, 100, quiet());
    w.clock = GameTime::from_dhm(1, 10, 0);
    let market = of_kind(&w, CarriageKind::Mercato)[0];
    let item = ItemKind::Vestito;
    let mut tokens = 5;
    assert_eq!(
        w.player_sell(market, item, &mut tokens),
        Err(SellError::NoMerchant)
    );
    staff_counter(&mut w, market);
    let serra = of_kind(&w, CarriageKind::Serra)[0];
    assert_eq!(
        w.player_sell(serra, ItemKind::Verdura, &mut tokens),
        Err(SellError::NotForSale)
    );
    assert_eq!(
        w.player_sell(market, ItemKind::Rottame, &mut tokens),
        Err(SellError::NotForSale)
    );
    let (supply, treasury) = (w.money_supply(), w.economy.treasury);
    let stock = w.carriages[market.index()].stock.get(item);
    let price = w.price(market, item).unwrap();
    let expected = w.sell_price(market, item).unwrap();
    assert!(expected > 0 && expected < price);
    let paid = w.player_sell(market, item, &mut tokens).unwrap();
    assert_eq!(paid, expected);
    assert_eq!(tokens, 5 + paid);
    assert_eq!(w.economy.treasury, treasury - u64::from(paid));
    // The tokens left the sim with the player.
    assert_eq!(w.money_supply(), supply - u64::from(paid));
    assert_eq!(w.economy.counters.player_sales, u64::from(paid));
    assert_eq!(w.carriages[market.index()].stock.get(item), stock + 1.0);
    assert!(matches!(
        w.events.last().unwrap().kind,
        EventKind::PlayerSold { item: ItemKind::Vestito, price, carriage } if price == paid && carriage == market
    ));
    assert!(
        w.events
            .last()
            .unwrap()
            .to_string()
            .contains("Hai venduto un vestito")
    );
    // A full shelf buys nothing; nor does an empty treasury.
    let cap = w.params.market_goods_cap;
    w.carriages[market.index()].stock.set(item, cap);
    assert_eq!(
        w.player_sell(market, item, &mut tokens),
        Err(SellError::NoRoom)
    );
    w.carriages[market.index()].stock.set(item, 0.0);
    w.economy.treasury = 0;
    let before = tokens;
    assert_eq!(
        w.player_sell(market, item, &mut tokens),
        Err(SellError::NoMoney)
    );
    assert_eq!(tokens, before);
    // Buying it back costs more than it paid: no free money.
    w.economy.treasury = treasury;
    w.carriages[market.index()].stock.set(item, stock);
    let mut tokens = 100;
    let sold = w.player_sell(market, item, &mut tokens).unwrap();
    let bought = w.player_buy(market, item, &mut tokens).unwrap();
    assert!(bought > sold, "sold {sold}, bought {bought}");
}

/// Every token stays in the sim, except those the player brings in by
/// buying and takes out by selling.
#[test]
fn money_is_conserved_with_player_trade() {
    let params = SimParams {
        theft_temptation_per_hour: 0.5,
        max_events: usize::MAX,
        ..SimParams::default()
    };
    let mut w = World::generate_with_params(8, 20, 300, params);
    let mut brain = UtilityBrain::new(8);
    let markets = of_kind(&w, CarriageKind::Mercato);
    let mut expected = w.money_supply();
    let mut player = 200u32;
    let (mut sold, mut bought) = (0, 0);
    for day in 0..12u64 {
        for minute in 0..DAY {
            w.run(&mut brain, 1);
            if minute % 97 == 0 && (10..17).contains(&w.clock.hour()) {
                let market = markets[(minute as usize / 97) % markets.len()];
                staff_counter(&mut w, market);
                let item = ItemKind::SOLD[(day as usize + minute as usize) % ItemKind::SOLD.len()];
                if minute % 2 == 0 {
                    if let Ok(price) = w.player_buy(market, item, &mut player) {
                        expected += u64::from(price);
                        bought += 1;
                    }
                } else if let Ok(pay) = w.player_sell(market, item, &mut player) {
                    expected -= u64::from(pay);
                    sold += 1;
                }
            }
            assert_eq!(w.money_supply(), expected, "day {day} at {}", w.clock);
        }
    }
    assert!(sold > 0 && bought > 0, "sold {sold}, bought {bought}");
    let c = &w.economy.counters;
    assert!(c.player_sales > 0 && c.player_purchases > 0);
}

#[test]
fn market_survives_save_and_load() {
    let mut a = World::generate(42, 20, 200);
    let mut brain_a = UtilityBrain::new(42);
    a.run(&mut brain_a, 3 * DAY + 77);
    let mut b: World = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
    let mut brain_b = brain_a.clone();
    let market = of_kind(&a, CarriageKind::Mercato)[0];
    assert_eq!(a.market_quotes(market), b.market_quotes(market));
    a.run(&mut brain_a, 2 * DAY);
    b.run(&mut brain_b, 2 * DAY);
    assert_eq!(a.price_history(), b.price_history());
    for c in &a.carriages {
        assert_eq!(a.specialties(c.id), b.specialties(c.id));
    }
}
