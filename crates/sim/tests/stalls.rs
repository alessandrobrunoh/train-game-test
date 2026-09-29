//! Banchi del Mercato: inventari degli NPC, messa in vendita di qualsiasi
//! oggetto, acquisti tra NPC e giocatore, scadenze, eredità.

use sim::{
    Action, Buyer, CarriageId, GiveError, ItemKind, MINUTES_PER_DAY, NPC_ITEM_SLOTS, OfferSource,
    Seller, SimParams, StallError, StallEvent, UtilityBrain, World,
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
        World::generate_with_params(7, 10, 60, params),
        UtilityBrain::new(7),
    )
}

fn first_market(w: &World) -> CarriageId {
    w.markets()[0]
}

/// Puts the player in `market`.
fn player_to(w: &mut World, market: CarriageId) {
    w.set_player_place(sim::Place {
        carriage: market,
        floor: 0,
    });
}

/// Moves NPC index `i` to `market`, idle.
fn npc_to(w: &mut World, i: usize, market: CarriageId) {
    let n = &mut w.npcs[i];
    n.carriage = market;
    n.floor = 0;
    n.action = Action::Idle;
}

/// Units of `item` held by the player, the NPCs and on the stalls.
fn held(w: &World, item: ItemKind) -> u32 {
    let npcs: u32 = w.npcs.iter().map(|n| n.inventory.items.count(item)).sum();
    let stalls: u32 = w
        .listings()
        .iter()
        .filter(|l| l.item == item)
        .map(|l| l.qty)
        .sum();
    w.player.inventory.count(item) + w.player.chest.count(item) + npcs + stalls
}

/// The trade pool only moves through its counted flows.
fn assert_pool(w: &World, start: u64, start_in: u64, start_out: u64) {
    let t = w.trade_counters();
    assert_eq!(
        w.trade_pool_units() + (t.out_of_pool - start_out),
        start + (t.into_pool - start_in),
        "trade pool at {}: {t:?}",
        w.clock
    );
}

#[test]
fn npcs_carry_a_few_slots_of_anything() {
    let (w, _) = world(quiet());
    for n in &w.npcs {
        assert_eq!(n.inventory.items.len(), NPC_ITEM_SLOTS);
        assert!(n.inventory.items.is_empty());
    }
}

#[test]
fn listing_buying_and_withdrawing_conserve_money_and_goods() {
    let (mut w, _) = world(quiet());
    let market = first_market(&w);
    player_to(&mut w, market);
    w.player.inventory.add(ItemKind::Verdura, 5);
    npc_to(&mut w, 0, market);
    w.npcs[0].inventory.tokens = 10;
    let supply = w.money_supply();
    let pool = w.trade_pool_units();
    let (pin, pout) = (w.trade_counters().into_pool, w.trade_counters().out_of_pool);
    let goods = held(&w, ItemKind::Verdura);

    let id = w
        .player_list_for_sale(market, ItemKind::Verdura, 5, 2)
        .unwrap();
    assert_eq!(w.player.inventory.count(ItemKind::Verdura), 0);
    let l = w.listing(id).unwrap();
    assert_eq!((l.qty, l.price_each, l.seller), (5, 2, Seller::Player));
    assert_eq!(w.listings_at(market).len(), 1);
    assert_eq!(w.seller_name(Seller::Player), w.player.name);
    assert!(matches!(
        w.stall_log().back().unwrap().event,
        StallEvent::Listed { qty: 5, .. }
    ));
    assert_eq!(held(&w, ItemKind::Verdura), goods);
    assert_pool(&w, pool, pin, pout);

    // An NPC at the Mercato buys two: it pays the player directly.
    let buyer = w.npcs[0].id;
    let player_tokens = w.player.tokens;
    let sale = w.buy_listing(Buyer::Npc(buyer), id, 2).unwrap();
    assert_eq!((sale.units, sale.paid), (2, 4));
    assert_eq!(w.player.tokens, player_tokens + 4);
    assert_eq!(w.npcs[0].inventory.tokens, 6);
    assert_eq!(w.npcs[0].inventory.items.count(ItemKind::Verdura), 2);
    assert_eq!(w.listing(id).unwrap().qty, 3);
    assert_eq!(w.money_supply(), supply);
    assert_eq!(held(&w, ItemKind::Verdura), goods);
    assert_pool(&w, pool, pin, pout);
    let t = w.trade_counters();
    assert_eq!((t.trades, t.sold_units, t.sold_tokens), (1, 2, 4));

    // Too expensive, own goods, not there: nothing changes.
    let (tokens, player_tokens) = (w.npcs[0].inventory.tokens, w.player.tokens);
    w.npcs[0].inventory.tokens = 1;
    w.player.tokens += tokens - 1;
    assert_eq!(
        w.buy_listing(Buyer::Npc(buyer), id, 1),
        Err(StallError::TooExpensive(2))
    );
    assert_eq!(w.player_buy_listing(id, 1), Err(StallError::OwnListing));
    w.npcs[1].carriage = CarriageId(0);
    let far = w.npcs[1].id;
    assert_eq!(
        w.buy_listing(Buyer::Npc(far), id, 1),
        Err(StallError::NotThere)
    );

    w.npcs[0].inventory.tokens = tokens;
    w.player.tokens = player_tokens;
    // The player takes the rest back.
    assert_eq!(w.player_withdraw(id), Ok(3));
    assert!(w.listing(id).is_none());
    assert_eq!(w.player.inventory.count(ItemKind::Verdura), 3);
    assert_eq!(w.money_supply(), supply);
    assert_eq!(held(&w, ItemKind::Verdura), goods);
    assert_pool(&w, pool, pin, pout);
    assert_eq!(w.player_withdraw(id), Err(StallError::NoSuchListing));
}

#[test]
fn any_item_can_be_listed() {
    let (mut w, _) = world(quiet());
    let market = first_market(&w);
    player_to(&mut w, market);
    // Food, a material, a comfort good, a tool.
    let goods = [
        ItemKind::Razione,
        ItemKind::Metallo,
        ItemKind::Coperta,
        ItemKind::Attrezzo,
    ];
    for item in goods {
        w.player.inventory.add(item, 1);
        let price = w.quote(market, item).unwrap();
        assert!(price >= 1);
        let id = w.player_list_for_sale(market, item, 1, price).unwrap();
        assert_eq!(w.listing(id).unwrap().item, item);
    }
    // At most `max_listings_per_seller`.
    w.player.inventory.add(ItemKind::Te, 1);
    assert_eq!(
        w.player_list_for_sale(market, ItemKind::Te, 1, 1),
        Err(StallError::TooManyListings)
    );
    assert_eq!(
        w.player_list_for_sale(market, ItemKind::Te, 2, 1),
        Err(StallError::NotOwned)
    );
    assert_eq!(
        w.player_list_for_sale(market, ItemKind::Te, 0, 1),
        Err(StallError::BadQuantity)
    );
    // Not a Mercato, or not there.
    assert_eq!(
        w.player_list_for_sale(CarriageId(0), ItemKind::Te, 1, 1),
        Err(StallError::NotAMarket)
    );

    // An NPC lists what it carries, too.
    npc_to(&mut w, 3, market);
    w.npcs[3].inventory.items.add(ItemKind::Tessuto, 4);
    let seller = w.npcs[3].id;
    let id = w
        .list_for_sale(Seller::Npc(seller), market, ItemKind::Tessuto, 4, 5)
        .unwrap();
    assert!(w.npcs[3].inventory.items.is_empty());

    // Every item has a quote there, with its offers cheapest first.
    let offers = w.market_offers(market);
    assert_eq!(offers.len(), ItemKind::COUNT);
    for o in &offers {
        assert!(o.quote >= 1, "{o:?}");
        assert!(o.offers.windows(2).all(|p| p[0].price <= p[1].price));
    }
    let tessuto = offers.iter().find(|o| o.item == ItemKind::Tessuto).unwrap();
    assert_eq!(tessuto.best().unwrap().source, OfferSource::Listing(id));
    assert_eq!(w.market_offers(CarriageId(0)), Vec::new());
    // Quotes of what the shelf deals in are its prices.
    for item in ItemKind::ALL.into_iter().filter(|i| i.is_sold()) {
        assert_eq!(w.quote(market, item), w.price(market, item));
    }
}

#[test]
fn expired_listings_go_back_to_their_seller() {
    let (mut w, mut brain) = world(quiet());
    let market = first_market(&w);
    player_to(&mut w, market);
    // Metallo: nobody buys it.
    w.player.inventory.add(ItemKind::Metallo, 3);
    let mine = w
        .player_list_for_sale(market, ItemKind::Metallo, 3, 50)
        .unwrap();
    npc_to(&mut w, 2, market);
    w.npcs[2].inventory.items.add(ItemKind::Metallo, 2);
    let seller = w.npcs[2].id;
    let theirs = w
        .list_for_sale(Seller::Npc(seller), market, ItemKind::Metallo, 2, 60)
        .unwrap();
    let supply = w.money_supply();
    w.run(&mut brain, w.params.listing_days * DAY - 60);
    assert!(w.listing(mine).is_some() && w.listing(theirs).is_some());
    w.run(&mut brain, 2 * 60);
    assert!(w.listing(mine).is_none() && w.listing(theirs).is_none());
    assert_eq!(w.player.inventory.count(ItemKind::Metallo), 3);
    // The NPC got its goods back (it may have listed them again since).
    let npc = w.npc(seller).unwrap();
    let relisted: u32 = w
        .listings_of(Seller::Npc(seller))
        .iter()
        .filter(|l| l.item == ItemKind::Metallo)
        .map(|l| l.qty)
        .sum();
    assert_eq!(npc.inventory.items.count(ItemKind::Metallo) + relisted, 2);
    let expired = w
        .stall_log()
        .iter()
        .filter(|r| matches!(r.event, StallEvent::ListingExpired { to_mercato: 0, .. }))
        .count();
    assert!(expired >= 2, "{:?}", w.stall_log());
    assert_eq!(w.money_supply(), supply);
}

#[test]
fn a_full_inventory_gets_back_what_fits_the_rest_goes_to_the_chest() {
    let (mut w, mut brain) = world(quiet());
    let market = first_market(&w);
    player_to(&mut w, market);
    w.player.inventory.add(ItemKind::Metallo, 10);
    let id = w
        .player_list_for_sale(market, ItemKind::Metallo, 10, 99)
        .unwrap();
    // Fill the inventory with something else.
    w.player.inventory.add(ItemKind::Attrezzo, 99);
    assert_eq!(w.player_withdraw(id), Err(StallError::NoRoom));
    w.run(&mut brain, w.params.listing_days * DAY + 60);
    assert!(w.listing(id).is_none());
    assert_eq!(w.player.chest.count(ItemKind::Metallo), 10);
}

#[test]
fn a_dead_sellers_listing_goes_to_the_mercato() {
    let params = SimParams {
        mortality_base: 1e-12,
        mortality_growth: 0.2,
        ..quiet()
    };
    let (mut w, mut brain) = world(params);
    let market = first_market(&w);
    let i = w
        .npcs
        .iter()
        .position(|n| n.partner().is_none() && n.children().next().is_none() && n.job.is_none())
        .unwrap();
    npc_to(&mut w, i, market);
    let id = w.npcs[i].id;
    w.npcs[i].inventory.items.add(ItemKind::Metallo, 3);
    w.npcs[i].inventory.items.add(ItemKind::Coperta, 1);
    let listing = w
        .list_for_sale(Seller::Npc(id), market, ItemKind::Metallo, 3, 9)
        .unwrap();
    let metallo = held(&w, ItemKind::Metallo);
    let supply = w.money_supply();
    // Turns 200 at the coming midnight: certain death.
    let midnight = w.clock.next_at(0, 0).0 as i64;
    let year = w.params.minutes_per_year() as i64;
    w.npcs[i].born = midnight - 200 * year - 1;
    w.npcs[i].age = 199;
    let before_midnight = w.clock.next_at(0, 0).0 - w.clock.0 - 1;
    w.run(&mut brain, before_midnight);
    assert!(w.npc(id).is_some());
    w.run(&mut brain, 2);
    assert!(w.npc(id).is_none());

    // Its listing is now the Mercato's, same goods.
    assert!(w.listing(listing).is_none());
    let l = w
        .listings_at(market)
        .into_iter()
        .find(|l| l.item == ItemKind::Metallo)
        .unwrap()
        .clone();
    assert_eq!((l.seller, l.qty), (Seller::Mercato, 3));
    assert_eq!(held(&w, ItemKind::Metallo), metallo);
    // Its Coperta went to its home's storage (or, if full, to the Mercato).
    let t = w.trade_counters();
    assert_eq!(t.estate_units + t.to_mercato_units, 1 + 3, "{t:?}");
    // Buying the Mercato's goods pays the treasury.
    npc_to(&mut w, 0, market);
    assert_eq!(w.money_supply(), supply);
    w.npcs[0].inventory.tokens = 100;
    let supply = w.money_supply();
    let treasury = w.economy.treasury;
    let buyer = w.npcs[0].id;
    let sale = w.buy_listing(Buyer::Npc(buyer), l.id, 1).unwrap();
    assert_eq!(w.economy.treasury, treasury + u64::from(sale.paid));
    assert_eq!(w.money_supply(), supply);
    // Nobody can take it back: unsold, it goes to the train's storage.
    assert_eq!(w.withdraw(l.id), Err(StallError::NotYours));
}

#[test]
fn npcs_sell_their_surplus_and_others_buy_it() {
    let (mut w, mut brain) = (World::generate(11, 20, 200), UtilityBrain::new(11));
    let supply = w.money_supply();
    let pool = w.trade_pool_units();
    let (pin, pout) = (w.trade_counters().into_pool, w.trade_counters().out_of_pool);
    for _ in 0..30 {
        w.run(&mut brain, DAY);
        assert_eq!(w.money_supply(), supply, "money on {}", w.clock);
        assert_pool(&w, pool, pin, pout);
    }
    let t = w.trade_counters().clone();
    assert!(t.own_share_units > 0, "{t:?}");
    assert!(t.listings > 0 && t.npc_trades > 0, "{t:?}");
    assert!(t.sold_tokens > 0);
    let carrying = w
        .npcs
        .iter()
        .filter(|n| !n.inventory.items.is_empty())
        .count();
    assert!(carrying > 0);
    let sales = w
        .stall_log()
        .iter()
        .filter(|r| {
            matches!(
                r.event,
                StallEvent::SoldAtStall {
                    seller: Seller::Npc(_),
                    buyer: Buyer::Npc(_),
                    ..
                }
            )
        })
        .count();
    assert!(sales > 0);
    // Nobody starved.
    assert_eq!(
        w.life.deaths_by_cause[sim::DeathCause::Starvation.index()],
        0
    );
}

#[test]
fn the_player_lists_and_an_npc_buys_it() {
    let (mut w, mut brain) = (World::generate(3, 10, 120), UtilityBrain::new(3));
    let market = first_market(&w);
    player_to(&mut w, market);
    w.player.inventory.add(ItemKind::Vestito, 1);
    let id = w
        .player_list_for_sale(market, ItemKind::Vestito, 1, 1)
        .unwrap();
    // Plenty of people without clothes, and an empty shelf.
    for n in w.npcs.iter_mut().filter(|n| n.inventory.tokens >= 5) {
        n.inventory.clothes = None;
    }
    w.carriages[market.index()]
        .stock
        .set(ItemKind::Vestito, 0.0);
    let supply = w.money_supply();
    let tokens = w.player.tokens;
    let mut sold = false;
    for _ in 0..3 * 24 {
        w.run(&mut brain, 60);
        if w.listing(id).is_none() {
            sold = true;
            break;
        }
    }
    assert!(sold, "nobody bought the player's Vestito");
    assert_eq!(w.player.tokens, tokens + 1);
    assert_eq!(w.money_supply(), supply);
    assert!(w.stall_log().iter().any(|r| matches!(
        r.event,
        StallEvent::SoldAtStall {
            seller: Seller::Player,
            buyer: Buyer::Npc(_),
            ..
        }
    )));
}

#[test]
fn buying_at_an_npcs_stall_pleases_it() {
    let (mut w, _) = world(quiet());
    let market = first_market(&w);
    player_to(&mut w, market);
    npc_to(&mut w, 4, market);
    w.npcs[4].inventory.items.add(ItemKind::Te, 2);
    let seller = w.npcs[4].id;
    let id = w
        .list_for_sale(Seller::Npc(seller), market, ItemKind::Te, 2, 3)
        .unwrap();
    let before = w.npcs[4].player_affinity();
    let tokens = w.npcs[4].inventory.tokens;
    assert_eq!(
        w.player_buy_listing(id, 5).map(|s| (s.units, s.paid)),
        Ok((2, 6))
    );
    assert!(w.npcs[4].player_affinity() > before);
    assert_eq!(w.npcs[4].inventory.tokens, tokens + 6);
    assert_eq!(w.player.inventory.count(ItemKind::Te), 2);
    assert!(w.listing(id).is_none());
    // The player can't withdraw someone else's goods.
    w.npcs[4].inventory.items.add(ItemKind::Te, 1);
    let other = w
        .list_for_sale(Seller::Npc(seller), market, ItemKind::Te, 1, 3)
        .unwrap();
    assert_eq!(w.player_withdraw(other), Err(StallError::NotYours));
}

#[test]
fn npcs_keep_gifts_they_can_sell() {
    let (mut w, _) = world(quiet());
    w.player.inventory.add(ItemKind::Metallo, 2);
    w.player.inventory.add(ItemKind::Rottame, 1);
    let id = w.npcs[0].id;
    let options = w.chat_gift_options(id);
    assert!(options.contains(&ItemKind::Metallo), "{options:?}");
    assert!(!options.contains(&ItemKind::Rottame), "{options:?}");
    assert_eq!(w.player_give(id, ItemKind::Metallo), Ok(()));
    assert_eq!(w.npcs[0].inventory.items.count(ItemKind::Metallo), 1);
    // Worth too little to bother.
    assert_eq!(
        w.player_give(id, ItemKind::Rottame),
        Err(GiveError::NotWanted)
    );
    assert_eq!(w.trade_counters().gifts_kept, 1);
}

#[test]
fn a_spare_tool_goes_on_when_the_old_one_breaks() {
    let (mut w, mut brain) = world(quiet());
    let i = w
        .npcs
        .iter()
        .position(|n| n.job.is_some_and(sim::Job::uses_tool) && n.inventory.tool.is_some())
        .unwrap();
    let id = w.npcs[i].id;
    w.npcs[i].inventory.tool = Some(0.0001);
    w.npcs[i].inventory.items.add(ItemKind::Attrezzo, 1);
    // Work until the old one breaks.
    for _ in 0..3 {
        w.run(&mut brain, DAY);
    }
    let n = w.npc(id).unwrap();
    assert!(n.inventory.tool.is_some());
    assert!(w.trade_counters().equipped_units >= 1);
}

#[test]
fn npc_belongings_and_stalls_survive_save_and_load() {
    let mut a = World::generate(42, 20, 200);
    let mut brain_a = UtilityBrain::new(42);
    a.run(&mut brain_a, 5 * DAY + 77);
    a.npcs[0].inventory.items.add(ItemKind::Coperta, 1);
    let json = serde_json::to_string(&a).unwrap();
    let mut b: World = serde_json::from_str(&json).unwrap();
    let mut brain_b = brain_a.clone();
    assert_eq!(a.npcs[0].inventory, b.npcs[0].inventory);
    assert_eq!(a.listings(), b.listings());
    assert_eq!(a.trade_counters(), b.trade_counters());
    a.run(&mut brain_a, 3 * DAY);
    b.run(&mut brain_b, 3 * DAY);
    assert_eq!(a.listings(), b.listings());
    assert_eq!(a.stall_log(), b.stall_log());
    for (x, y) in a.npcs.iter().zip(&b.npcs) {
        assert_eq!(x.inventory, y.inventory);
    }

    // A save from before the NPCs carried anything loads with empty slots.
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    for n in value["npcs"].as_array_mut().unwrap() {
        n["inventory"].as_object_mut().unwrap().remove("items");
    }
    value.as_object_mut().unwrap().remove("trade_rng");
    let old: World = serde_json::from_value(value).unwrap();
    assert!(
        old.npcs
            .iter()
            .all(|n| n.inventory.items.len() == NPC_ITEM_SLOTS)
    );
    assert!(old.npcs[0].inventory.items.is_empty());
}

#[test]
fn trades_are_deterministic() {
    let run = || {
        let mut w = World::generate(5, 20, 200);
        let mut brain = UtilityBrain::new(5);
        w.run(&mut brain, 12 * DAY);
        w
    };
    let (a, b) = (run(), run());
    assert!(a.trade_counters().trades > 0, "{:?}", a.trade_counters());
    assert_eq!(a.trade_counters(), b.trade_counters());
    assert_eq!(a.stall_log(), b.stall_log());
    assert_eq!(a.listings(), b.listings());
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn stall_texts_are_italian() {
    let (mut w, _) = world(quiet());
    let market = first_market(&w);
    player_to(&mut w, market);
    w.set_player_name("Ada");
    w.player.inventory.add(ItemKind::Razione, 2);
    let id = w
        .player_list_for_sale(market, ItemKind::Razione, 2, 3)
        .unwrap();
    npc_to(&mut w, 1, market);
    w.npcs[1].inventory.tokens = 50;
    let buyer = w.npcs[1].id;
    w.buy_listing(Buyer::Npc(buyer), id, 1).unwrap();
    let texts: Vec<String> = w.stall_log().iter().map(|r| r.to_string()).collect();
    assert!(
        texts[0].contains("Ada mette sul banco 2 razioni a 3 gettoni"),
        "{texts:?}"
    );
    assert!(
        texts[1].contains("compra una razione al banco di Ada per 3 gettoni"),
        "{texts:?}"
    );
    assert_eq!(w.stall_log_total(), 2);
}
