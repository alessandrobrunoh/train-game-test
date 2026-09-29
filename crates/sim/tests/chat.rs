//! La chat del giocatore (Fase 5.4): risposte, affinità con i tempi di
//! attesa, testo libero, incarichi pagati dai gettoni dell'NPC, memoria nei
//! salvataggi e determinismo.

use sim::chat::{
    CHAT_BONUS_COOLDOWN_MINUTES, CHAT_GREET_AFFINITY, FAVOUR_AFFINITY, INSULT_AFFINITY,
    INSULT_COOLDOWN_MINUTES,
};
use sim::{
    Action, CHAT_MEMORY_LINES, CarriageKind, ChatAction, ChatError, IntentReader, ItemKind, Job,
    KeywordReader, NpcId, Personality, Place, PlayerTie, Speaker, Temper, UtilityBrain, World,
};
use sim::{Intent, Npc};

fn snapshot(w: &World) -> String {
    serde_json::to_string(w).expect("world serializes")
}

/// A world and an awake adult (not a Mercante) idling where the player is.
fn setup(seed: u64) -> (World, UtilityBrain, NpcId) {
    let mut w = World::generate(seed, 10, 200);
    let brain = UtilityBrain::new(seed);
    let i = w
        .npcs
        .iter()
        .position(|n| n.age >= 25 && n.age < 60 && n.job.is_some() && n.job != Some(Job::Mercante))
        .expect("an adult worker");
    let id = w.npcs[i].id;
    idle_near_player(&mut w, i, 600);
    (w, brain, id)
}

fn idle_near_player(w: &mut World, i: usize, minutes: u64) {
    let now = w.clock;
    let n = &mut w.npcs[i];
    if let Some(s) = n.action.station() {
        let st = &mut w.carriages[n.carriage.index()].stations[s.index()];
        st.occupancy = st.occupancy.saturating_sub(1);
    }
    n.floor = 0;
    n.action = Action::Idle;
    n.action_since = now;
    n.action_until = now + minutes;
    let place = Place {
        carriage: n.carriage,
        floor: 0,
    };
    w.set_player_place(place);
}

fn npc(w: &World, id: NpcId) -> &Npc {
    w.npc(id).unwrap()
}

fn index(w: &World, id: NpcId) -> usize {
    w.npcs.iter().position(|n| n.id == id).unwrap()
}

#[test]
fn talking_needs_the_npc_near_and_awake() {
    let (mut w, _, id) = setup(1);
    assert_eq!(w.can_chat(id), Ok(()));
    assert_eq!(w.can_chat(NpcId(99_999)), Err(ChatError::NoSuchNpc));
    let away = w
        .carriages
        .iter()
        .find(|c| c.id != npc(&w, id).carriage)
        .unwrap()
        .id;
    w.set_player_place(Place {
        carriage: away,
        floor: 0,
    });
    assert_eq!(w.player_chat(id, Intent::Greet), Err(ChatError::Away));
    let i = index(&w, id);
    idle_near_player(&mut w, i, 60);
    let bed = w.carriages[npc(&w, id).carriage.index()]
        .stations
        .first()
        .map(|s| s.id)
        .unwrap();
    w.npcs[i].action = Action::Sleep(bed);
    assert_eq!(w.player_chat(id, Intent::Greet), Err(ChatError::Asleep));
    assert!(w.chat_log(id).is_empty());
}

#[test]
fn greetings_and_insults_have_a_cooldown() {
    let (mut w, mut brain, id) = setup(2);
    assert!(npc(&w, id).player.is_none());
    let reply = w.player_chat(id, Intent::Greet).unwrap();
    assert_eq!(reply.intent, Some(Intent::Greet));
    assert!(!reply.said.is_empty() && !reply.answer.is_empty());
    // A stranger learns the player's name, and the tie starts.
    assert!(reply.said.contains(w.player.first_name()), "{}", reply.said);
    assert!((npc(&w, id).player_affinity() - CHAT_GREET_AFFINITY).abs() < 1e-6);
    // Spamming greetings farms nothing.
    for _ in 0..10 {
        let again = w.player_chat(id, Intent::Greet).unwrap();
        assert_eq!(again.affinity, 0.0);
    }
    assert!((npc(&w, id).player_affinity() - CHAT_GREET_AFFINITY).abs() < 1e-6);
    // Later it counts again.
    let i = index(&w, id);
    w.run(&mut brain, CHAT_BONUS_COOLDOWN_MINUTES);
    idle_near_player(&mut w, i, 600);
    w.player_chat(id, Intent::Greet).unwrap();
    assert!((npc(&w, id).player_affinity() - 2.0 * CHAT_GREET_AFFINITY).abs() < 1e-6);

    // An insult hurts once per cooldown.
    let before = npc(&w, id).player_affinity();
    let insult = w.player_chat(id, Intent::Insult).unwrap();
    assert!((insult.affinity - INSULT_AFFINITY).abs() < 1e-6);
    for _ in 0..5 {
        assert_eq!(w.player_chat(id, Intent::Insult).unwrap().affinity, 0.0);
    }
    assert!((npc(&w, id).player_affinity() - (before + INSULT_AFFINITY)).abs() < 1e-6);
    w.run(&mut brain, INSULT_COOLDOWN_MINUTES);
    idle_near_player(&mut w, i, 600);
    w.player_chat(id, Intent::Insult).unwrap();
    assert!((npc(&w, id).player_affinity() - (before + 2.0 * INSULT_AFFINITY)).abs() < 1e-5);
}

#[test]
fn every_suggested_reply_gets_an_answer() {
    let (mut w, _, id) = setup(3);
    for intent in Intent::ALL {
        let reply = w.player_chat(id, intent).unwrap();
        assert_eq!(reply.intent, Some(intent));
        assert!(!reply.answer.is_empty(), "{intent:?}");
        assert!(!reply.answer.contains('{'), "{intent:?}: {}", reply.answer);
    }
    assert_eq!(
        w.player_chat(id, Intent::Farewell).unwrap().action,
        ChatAction::End
    );
    // Both lines of every exchange are remembered.
    let log = w.chat_log(id);
    assert_eq!(log.len(), 2 * (Intent::ALL.len() + 1));
    assert_eq!(log[0].speaker, Speaker::Player);
    assert_eq!(log[1].speaker, Speaker::Npc);
}

#[test]
fn free_text_is_understood_or_not() {
    let (mut w, _, id) = setup(4);
    let reader = KeywordReader;
    let reply = w
        .player_chat_text(id, "  Ciao,   che lavoro fai? ", &reader)
        .unwrap();
    assert_eq!(reply.intent, Some(Intent::AskJob));
    assert_eq!(reply.said, "Ciao, che lavoro fai?");
    assert!(!reply.answer.is_empty());
    let before = npc(&w, id).player_affinity();
    let huh = w.player_chat_text(id, "zxcv qwerty", &reader).unwrap();
    assert_eq!(huh.intent, None);
    assert_eq!(huh.affinity, 0.0);
    assert_eq!(huh.action, ChatAction::None);
    assert_eq!(npc(&w, id).player_affinity(), before);
    let log = w.chat_log(id);
    assert_eq!(log[log.len() - 2].text, "zxcv qwerty");
    assert_eq!(log[log.len() - 1].speaker, Speaker::Npc);
    // A different reader can take over (e.g. a model).
    struct Always(Intent);
    impl IntentReader for Always {
        fn read(&self, _: &str) -> Option<Intent> {
            Some(self.0)
        }
    }
    let r = w
        .player_chat_text(id, "qualsiasi cosa", &Always(Intent::AskNews))
        .unwrap();
    assert_eq!(r.intent, Some(Intent::AskNews));
}

#[test]
fn a_favour_is_paid_from_the_npcs_tokens() {
    let (mut w, _, id) = setup(5);
    let i = index(&w, id);
    w.npcs[i].inventory.clothes = None;
    w.npcs[i].inventory.tokens = 60;
    w.npcs[i].player = Some(PlayerTie {
        affinity: 0.3,
        ..PlayerTie::default()
    });
    let money = w.money_supply();
    let offer = w.player_chat(id, Intent::AskFavour).unwrap();
    let favour = offer.favour.expect("a favour");
    assert!(npc(&w, id).wants(favour.item), "{favour:?}");
    assert!(favour.reward > 0 && favour.reward <= 30, "{favour:?}");
    assert!(
        offer.answer.contains(&format!("{} gettoni", favour.reward)),
        "{}",
        offer.answer
    );
    assert_eq!(w.player_favour(id), Some(favour));
    // Without the item: a reminder, nothing paid.
    let remind = w.player_chat(id, Intent::AskFavour).unwrap();
    assert_eq!(remind.tokens, 0);
    assert_eq!(w.player_favour(id), Some(favour));
    // With it: done, paid by the NPC, money conserved.
    w.player.inventory.add(favour.item, favour.count);
    let tokens = w.player.tokens;
    let affinity = npc(&w, id).player_affinity();
    let done = w.player_chat(id, Intent::AskFavour).unwrap();
    assert_eq!(done.tokens, favour.reward);
    assert!(
        done.said.contains(favour.item.with_article()),
        "{}",
        done.said
    );
    assert_eq!(w.player.tokens, tokens + favour.reward);
    assert_eq!(npc(&w, id).inventory.tokens, 60 - favour.reward);
    assert_eq!(w.player.inventory.count(favour.item), 0);
    assert!(npc(&w, id).inventory.has(favour.item));
    assert_eq!(w.player_favour(id), None);
    assert!((npc(&w, id).player_affinity() - (affinity + FAVOUR_AFFINITY)).abs() < 1e-6);
    assert_eq!(w.money_supply(), money);

    // A broke NPC promises only what it has.
    w.npcs[i].inventory.tokens = 0;
    w.npcs[i].inventory.tool = None;
    w.npcs[i].inventory.clothes = None;
    let poor = w.player_chat(id, Intent::AskFavour).unwrap();
    assert_eq!(poor.favour.map(|f| f.reward), Some(0));
    // Who distrusts the player asks nothing.
    let (mut w2, _, id2) = setup(5);
    let k = index(&w2, id2);
    w2.npcs[k].inventory.clothes = None;
    w2.npcs[k].player = Some(PlayerTie {
        affinity: -0.8,
        ..PlayerTie::default()
    });
    assert_eq!(w2.player_chat(id2, Intent::AskFavour).unwrap().favour, None);
}

#[test]
fn a_friend_with_a_favour_to_ask_speaks_first() {
    let (mut w, mut brain, id) = setup(6);
    let i = index(&w, id);
    w.npcs[i].inventory.clothes = None;
    w.npcs[i].inventory.tokens = 40;
    w.npcs[i].player = Some(PlayerTie {
        affinity: 0.8,
        ..PlayerTie::default()
    });
    // The greetings come every few minutes.
    for _ in 0..10 {
        w.run(&mut brain, 1);
        if w.player_favour(id).is_some() {
            break;
        }
    }
    let favour = w.player_favour(id).expect("an offer");
    assert!(!favour.told);
    assert!(w.wants_to_talk(id));
    let opening = w.player_chat_start(id).unwrap().expect("it speaks first");
    assert!(
        opening.contains(
            sim::dialogue::grammar::counted(
                favour.count,
                favour.item.with_article(),
                favour.item.plural()
            )
            .as_str()
        ),
        "{opening}"
    );
    assert!(!w.wants_to_talk(id));
    assert!(w.player_favour(id).unwrap().told);
    assert_eq!(w.chat_log(id).last().unwrap().text, opening);
    // Nothing more to say.
    assert_eq!(w.player_chat_start(id).unwrap(), None);
}

#[test]
fn prices_trade_gifts_and_gossip_use_what_the_npc_knows() {
    let (mut w, _, id) = setup(7);
    let i = index(&w, id);
    // Everyone near a Mercato knows its prices.
    let market = w.markets()[0];
    w.npcs[i].carriage = market;
    w.npcs[i].home = market;
    idle_near_player(&mut w, i, 600);
    let prices = w.player_chat(id, Intent::AskPrices).unwrap();
    assert!(prices.answer.contains("getton"), "{}", prices.answer);
    // Asking about an item by name: the answer is about it.
    for (text, item) in [
        ("quanto costa un vestito?", ItemKind::Vestito),
        ("e gli attrezzi, quanto costano?", ItemKind::Attrezzo),
    ] {
        let named = w.player_chat_text(id, text, &KeywordReader).unwrap();
        assert_eq!(named.intent, Some(Intent::AskPrices));
        assert!(
            named.answer.contains(item.name()) || named.answer.contains(item.plural()),
            "{text}: {}",
            named.answer
        );
    }

    // Not a merchant: it sends the player to a Mercato.
    let trade = w.player_chat(id, Intent::Trade).unwrap();
    assert_eq!(trade.action, ChatAction::None);

    // Gifts: the chat opens the choice only if it accepts something.
    w.npcs[i].needs.hunger = 0.2;
    assert_eq!(
        w.player_chat(id, Intent::Gift).unwrap().action,
        ChatAction::None
    );
    w.player.inventory.add(ItemKind::Razione, 1);
    assert_eq!(w.chat_gift_options(id), [ItemKind::Razione]);
    assert_eq!(
        w.player_chat(id, Intent::Gift).unwrap().action,
        ChatAction::OpenGift
    );
    let thanks = w.player_chat_gift(id, ItemKind::Razione).unwrap();
    assert!(thanks.affinity > 0.0);
    assert!(thanks.said.contains("razione"), "{}", thanks.said);
    assert!(npc(&w, id).needs.hunger > 0.5);

    // A gossip heard that the player took something where it lives.
    w.npcs[i].personality = Some(Personality::default().with(Temper::Pettegolo));
    let storage = w
        .carriages
        .iter()
        .find(|c| c.kind == CarriageKind::Officina)
        .unwrap()
        .id;
    assert_eq!(w.player_take(storage, ItemKind::Rottame, 1), 1);
    let news = w.player_chat(id, Intent::AskNews).unwrap();
    let place = &w.carriages[storage.index()].name;
    let head = place.split(' ').next_back().unwrap();
    assert!(news.answer.contains(head), "{} ({place})", news.answer);
}

#[test]
fn a_merchant_at_the_counter_opens_the_market() {
    let mut w = World::generate(8, 10, 200);
    let market = w.markets()[0];
    let i = w
        .npcs
        .iter()
        .position(|n| n.job == Some(Job::Mercante))
        .unwrap();
    let counter = w.carriages[market.index()]
        .free_station(sim::StationKind::Counter)
        .unwrap();
    let now = w.clock;
    let n = &mut w.npcs[i];
    if let Some(s) = n.action.station() {
        let st = &mut w.carriages[n.carriage.index()].stations[s.index()];
        st.occupancy = st.occupancy.saturating_sub(1);
    }
    n.carriage = market;
    n.workplace = Some(market);
    n.floor = 0;
    n.action = Action::Work(counter);
    n.action_until = now + 600;
    w.carriages[market.index()].stations[counter.index()].occupancy += 1;
    let id = w.npcs[i].id;
    w.set_player_place(Place {
        carriage: market,
        floor: 0,
    });
    let reply = w.player_chat(id, Intent::Trade).unwrap();
    assert_eq!(reply.action, ChatAction::OpenMarket(market));
    let prices = w.player_chat(id, Intent::AskPrices).unwrap();
    assert!(prices.answer.contains("getton"), "{}", prices.answer);
}

#[test]
fn chat_memory_is_capped_and_survives_save_and_load() {
    let (mut w, mut brain, id) = setup(9);
    for k in 0..(CHAT_MEMORY_LINES + 4) {
        let intent = Intent::ALL[k % 3];
        w.player_chat(id, intent).unwrap();
    }
    assert_eq!(w.chat_log(id).len(), CHAT_MEMORY_LINES);
    let saved = snapshot(&w);
    let mut loaded: World = serde_json::from_str(&saved).unwrap();
    assert_eq!(loaded.chat_log(id), w.chat_log(id));
    assert_eq!(loaded.player.chats, w.player.chats);
    // And it goes on identically.
    let mut brain2 = brain.clone();
    let reader = KeywordReader;
    for world in [&mut w, &mut loaded] {
        world.player_chat_text(id, "novità?", &reader).unwrap();
    }
    w.run(&mut brain, 120);
    loaded.run(&mut brain2, 120);
    assert_eq!(snapshot(&w), snapshot(&loaded));
}

/// The same chats at the same minutes give the same world.
#[test]
fn chats_keep_the_world_deterministic() {
    let run = || {
        let (mut w, mut brain, id) = setup(10);
        let reader = KeywordReader;
        let i = index(&w, id);
        w.npcs[i].inventory.clothes = None;
        let mut replies = Vec::new();
        for step in 0..6u64 {
            w.run(&mut brain, 7);
            let Some(k) = w.npcs.iter().position(|n| n.id == id) else {
                break;
            };
            idle_near_player(&mut w, k, 120);
            let _ = w.player_chat_start(id);
            let intent = Intent::ALL[(step as usize * 5) % Intent::ALL.len()];
            replies.push(w.player_chat(id, intent).unwrap().answer);
            replies.push(
                w.player_chat_text(id, "ciao, quanto costa un vestito?", &reader)
                    .unwrap()
                    .answer,
            );
        }
        w.run(&mut brain, 3 * 24 * 60);
        (snapshot(&w), replies)
    };
    let (a, ra) = run();
    let (b, rb) = run();
    assert_eq!(ra, rb);
    assert!(a == b, "worlds differ");
}
