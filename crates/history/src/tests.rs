use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use sim::{
    DeathCause, Event, EventKind, GameTime, ItemKind, MINUTES_PER_DAY, NpcId, Sex, SimParams,
    UtilityBrain, World,
};

use super::*;
use crate::sync::{Seed, SeedPerson, SyncBatch, YearSnapshot};

/// Temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("traingame-history-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn small_world(seed: u64) -> (World, UtilityBrain) {
    (World::generate(seed, 10, 100), UtilityBrain::new(seed))
}

fn run_days(world: &mut World, brain: &mut UtilityBrain, days: u64) {
    world.run(brain, days * MINUTES_PER_DAY);
}

#[test]
fn sync_is_idempotent_and_incremental() {
    let (mut world, mut brain) = small_world(1);
    let mut h = History::open_in_memory().unwrap();
    assert!(!h.is_seeded());
    let r = h.sync(&world).unwrap();
    assert!(r.seeded);
    assert_eq!(r.inserted, world.events_total());
    assert_eq!(h.counts().unwrap().people, world.npcs.len() as u64);

    run_days(&mut world, &mut brain, 3);
    let total = world.events_total();
    assert!(total > 0);
    let r = h.sync(&world).unwrap();
    assert!(!r.seeded);
    assert_eq!(r.gap, None);
    assert_eq!(h.synced(), total);
    assert_eq!(h.counts().unwrap().events, total);
    // Nothing new: nothing written.
    assert_eq!(h.sync(&world).unwrap().inserted, 0);
    // The same batch applied twice is harmless.
    let batch = SyncBatch::capture(&world, 0, false);
    assert_eq!(h.apply(&batch).unwrap().inserted, 0);
    assert_eq!(h.counts().unwrap().events, total);

    run_days(&mut world, &mut brain, 2);
    let r = h.sync(&world).unwrap();
    assert_eq!(r.inserted, world.events_total() - total);
    // Stored rows match the events still in memory.
    let stored: i64 = h
        .connection()
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored as u64, world.events_total());
    let last = world.events.last().unwrap();
    let text: String = h
        .connection()
        .query_row(
            "SELECT text FROM events WHERE seq = ?1",
            [world.events_total() as i64 - 1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(text, sync::event_text(last));
    assert!(!text.starts_with('['));
}

#[test]
fn records_a_gap_when_events_were_dropped_before_the_sync() {
    let params = SimParams {
        max_events: 40,
        ..SimParams::default()
    };
    let mut world = World::generate_with_params(3, 10, 100, params);
    let mut brain = UtilityBrain::new(3);
    let mut h = History::open_in_memory().unwrap();
    h.sync(&world).unwrap();
    run_days(&mut world, &mut brain, 10);
    let total = world.events_total();
    let oldest = total - world.events.len() as u64;
    assert!(oldest > 0, "the log must have been trimmed");
    let r = h.sync(&world).unwrap();
    assert_eq!(r.gap, Some((0, oldest)));
    let counts = h.counts().unwrap();
    assert_eq!(counts.gaps, 1);
    assert_eq!(counts.lost_events + counts.events, total);
    assert_eq!(h.synced(), total);
}

#[test]
fn rollback_forgets_the_future_and_the_replay_matches() {
    let (mut world, mut brain) = small_world(5);
    let mut h = History::open_in_memory().unwrap();
    run_days(&mut world, &mut brain, 30);
    h.sync(&world).unwrap();
    // "Save" here.
    let saved = (world.clone(), brain.clone());
    let saved_total = world.events_total();
    let saved_counts = h.counts().unwrap();
    let saved_births = world.life.births_total;

    // Play on (syncing daily, as the game would) and diverge from the save.
    for _ in 0..60 {
        run_days(&mut world, &mut brain, 1);
        h.sync(&world).unwrap();
    }
    assert!(world.events_total() > saved_total);
    assert!(world.life.births_total > saved_births || world.life.deaths_total > 0);

    // "Load" the save: the future that never happened is gone.
    h.rollback_to(saved_total).unwrap();
    assert_eq!(h.synced(), saved_total);
    let c = h.counts().unwrap();
    assert_eq!(c.events, saved_counts.events);
    assert_eq!(c.people, saved_counts.people);
    assert_eq!(c.alive, saved_counts.alive);
    assert_eq!(c.couples, saved_counts.couples);

    // Replaying from the save gives the same history as syncing straight.
    let (mut world2, mut brain2) = saved;
    for _ in 0..10 {
        run_days(&mut world2, &mut brain2, 1);
        h.sync(&world2).unwrap();
    }
    let mut straight = History::open_in_memory().unwrap();
    let (mut world3, mut brain3) = small_world(5);
    run_days(&mut world3, &mut brain3, 30);
    straight.sync(&world3).unwrap();
    for _ in 0..10 {
        run_days(&mut world3, &mut brain3, 1);
        straight.sync(&world3).unwrap();
    }
    assert_eq!(h.counts().unwrap(), straight.counts().unwrap());
    assert_eq!(h.count_kind("Born").unwrap(), world2.life.births_total);
}

#[test]
fn sync_and_reconcile_roll_back_a_world_from_another_branch() {
    let (mut world, mut brain) = small_world(9);
    let mut h = History::open_in_memory().unwrap();
    run_days(&mut world, &mut brain, 5);
    h.sync(&world).unwrap();
    let saved = world.clone();
    let saved_total = saved.events_total();
    run_days(&mut world, &mut brain, 5);
    h.sync(&world).unwrap();

    // A world behind the history: sync rolls back by itself.
    let r = h.sync(&saved).unwrap();
    assert_eq!(r.rolled_back_to, Some(saved_total));
    assert_eq!(h.synced(), saved_total);
    assert_eq!(h.counts().unwrap().events, saved_total);

    // Same length, different content (another branch): reconcile finds where.
    let mut other = saved.clone();
    let last = other.events.len() - 1;
    other.events[last] = Event {
        time: other.events[last].time,
        kind: EventKind::Shortage {
            item: ItemKind::Vestito,
        },
    };
    let r = h.reconcile(&other).unwrap();
    assert_eq!(r.rolled_back_to, Some(saved_total - 1));
    assert!(!r.unverifiable);
    // Identical worlds: nothing to do.
    let r = h.reconcile(&other).unwrap();
    assert_eq!(r, ReconcileReport::default());
    h.sync(&other).unwrap();
    assert_eq!(h.counts().unwrap().events, saved_total);
}

fn born(time: u64, npc: u32, sex: Sex, mother: u32, father: u32) -> Event {
    Event {
        time: GameTime(time),
        kind: EventKind::Born {
            npc: NpcId(npc),
            name: format!("Figlio{npc} Cognome{father}"),
            sex,
            mother: NpcId(mother),
            father: NpcId(father),
            mother_name: format!("Madre{mother}"),
            father_name: format!("Padre{father}"),
        },
    }
}

fn coupled(time: u64, a: u32, b: u32) -> Event {
    Event {
        time: GameTime(time),
        kind: EventKind::Coupled {
            npc: NpcId(a),
            name: format!("P{a}"),
            partner: NpcId(b),
            partner_name: format!("P{b}"),
            moved_to: None,
        },
    }
}

fn died(time: u64, npc: u32, age: u32, sex: Sex) -> Event {
    Event {
        time: GameTime(time),
        kind: EventKind::NpcDied {
            npc: NpcId(npc),
            name: format!("Morto{npc} Rossi"),
            cause: DeathCause::OldAge,
            age,
            sex,
        },
    }
}

fn founder(id: u32, sex: Sex) -> SeedPerson {
    SeedPerson {
        id: NpcId(id),
        name: format!("Fondatore{id} Cognome{id}"),
        sex,
        born: -30 * 12 * MINUTES_PER_DAY as i64,
        founder: true,
        mother: None,
        father: None,
    }
}

fn batch(first_seq: u64, events: Vec<Event>, seed: Option<Seed>) -> SyncBatch {
    let world_total = first_seq + events.len() as u64;
    SyncBatch {
        first_seq,
        time: events.last().map_or(0, |e| e.time.0),
        events,
        gap: None,
        world_total,
        founders: 5,
        seed,
        year: YearSnapshot {
            year: 1,
            population: 5,
            founders: 5,
            couples: 2,
            avg_age: 30.0,
        },
    }
}

#[test]
fn family_tree_spans_three_generations_and_keeps_the_dead() {
    use Sex::{Female as F, Male as M};
    let mut h = History::open_in_memory().unwrap();
    let seed = Seed {
        people: vec![
            founder(0, F),
            founder(1, M),
            founder(2, F),
            founder(3, M),
            founder(4, F),
        ],
        couples: vec![(NpcId(0), NpcId(1)), (NpcId(2), NpcId(3))],
    };
    let year = 12 * MINUTES_PER_DAY;
    let events = vec![
        born(10, 5, F, 0, 1),        // 5: daughter of 0 + 1
        born(20, 6, M, 2, 3),        // 6: son of 2 + 3
        born(30, 7, M, 0, 1),        // 7: son of 0 + 1
        coupled(20 * year, 5, 6),    // 5 + 6
        born(22 * year, 8, M, 5, 6), // 8: grandson of 0
        coupled(40 * year, 4, 8),    // 8 + founder 4
        born(42 * year, 9, F, 4, 8), // 9: great-granddaughter of 0
        died(50 * year, 0, 80, F),
        died(51 * year, 1, 81, M),
    ];
    h.apply(&batch(0, events, Some(seed))).unwrap();

    // Down from founder 0: 5, 7 -> 8 -> 9.
    let tree = h.family_tree(NpcId(0), 2, 3).unwrap().unwrap();
    assert!(!tree.person.is_alive());
    assert_eq!(tree.person.death_age, Some(80));
    assert_eq!(tree.person.death_cause, Some(DeathCause::OldAge));
    assert_eq!(tree.partners.len(), 1);
    assert_eq!(tree.partners[0].id, NpcId(1));
    let kids: Vec<NpcId> = tree.children.iter().map(|c| c.person.id).collect();
    assert_eq!(kids, [NpcId(5), NpcId(7)]);
    assert_eq!(tree.children[0].partners[0].id, NpcId(6));
    let grandson = &tree.children[0].children[0];
    assert_eq!(grandson.person.id, NpcId(8));
    assert_eq!(grandson.children[0].person.id, NpcId(9));
    assert_eq!(tree.len(), 5);
    // Depth is honoured.
    let shallow = h.family_tree(NpcId(0), 2, 1).unwrap().unwrap();
    assert!(shallow.children.iter().all(|c| c.children.is_empty()));

    // Up from 9: parents 4 (mother) and 8; grandparents 5 and 6 via 8.
    let tree = h.family_tree(NpcId(9), 2, 3).unwrap().unwrap();
    let parents: Vec<NpcId> = tree.parents.iter().map(|p| p.person.id).collect();
    assert_eq!(parents, [NpcId(4), NpcId(8)]);
    let grandparents: Vec<NpcId> = tree.parents[1]
        .parents
        .iter()
        .map(|p| p.person.id)
        .collect();
    assert_eq!(grandparents, [NpcId(5), NpcId(6)]);
    assert!(tree.parents[1].parents[0].parents.is_empty(), "only 2 up");

    // The dead are still searchable, and the couple ended.
    let found = h.search_people("Morto", 10).unwrap();
    assert!(
        found.is_empty(),
        "names come from the seed, not the death event"
    );
    let found = h.search_people("fondatore0", 10).unwrap();
    assert_eq!(found.len(), 1);
    assert!(!found[0].is_alive());
    let couples = h.couples_of(NpcId(1)).unwrap();
    assert_eq!(couples.len(), 1);
    assert!(couples[0].formed.is_none(), "seeded couple");
    assert_eq!(couples[0].ended, Some(GameTime(50 * year)));

    // Records.
    let most = h.most_children(3).unwrap();
    assert_eq!(most[0].1, 2);
    assert!([NpcId(0), NpcId(1)].contains(&most[0].0.id));
    let oldest = h.longest_lived(1).unwrap();
    assert_eq!(oldest[0].id, NpcId(1));
    let causes = h.deaths_by_cause().unwrap();
    assert_eq!(causes[DeathCause::OldAge.index()], (DeathCause::OldAge, 2));
    let years = h.yearly_counts(12).unwrap();
    assert_eq!(years.first().map(|y| (y.year, y.births)), Some((1, 3)));
    assert_eq!(years.iter().map(|y| y.births).sum::<u64>(), 5);
    assert_eq!(years.iter().map(|y| y.deaths).sum::<u64>(), 2);
    let families = h.largest_families(10).unwrap();
    assert!(families.iter().any(|(s, n, _)| s == "Cognome1" && *n == 3));

    // Biography of the grandfather-to-be 6 includes his son's birth.
    let bio = h.biography(NpcId(6)).unwrap();
    let kinds: Vec<&str> = bio.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["Born", "Coupled", "Born"]);
    assert!(matches!(
        bio[2].event_kind(),
        Some(EventKind::Born { npc: NpcId(8), .. })
    ));

    // Rolling back before 9's birth forgets it, and 0 and 1 live again.
    h.rollback_to(6).unwrap();
    assert!(h.person(NpcId(9)).unwrap().is_none());
    assert!(h.person(NpcId(0)).unwrap().unwrap().is_alive());
    assert_eq!(h.couples_of(NpcId(8)).unwrap()[0].ended, None);
    assert_eq!(h.couples_of(NpcId(1)).unwrap()[0].ended, None);
}

#[test]
fn player_deeds_are_logged() {
    let (mut world, mut brain) = small_world(4);
    let mut h = History::open_in_memory().unwrap();
    run_days(&mut world, &mut brain, 1);
    let serra = world
        .carriages
        .iter()
        .find(|c| c.stock.count(ItemKind::Verdura) >= 3)
        .map(|c| c.id)
        .unwrap();
    assert_eq!(world.player_take(serra, ItemKind::Verdura, 3), 3);
    let hungry = world
        .npcs
        .iter()
        .find(|n| n.accepts_gift(ItemKind::Verdura))
        .map(|n| n.id);
    if let Some(id) = hungry {
        world.player_give(id, ItemKind::Verdura).unwrap();
    }
    h.sync(&world).unwrap();
    let deeds = h.player_actions(10).unwrap();
    assert_eq!(deeds.len(), 1 + usize::from(hungry.is_some()));
    let took = deeds.last().unwrap();
    assert_eq!(took.kind, "PlayerTook");
    assert_eq!(took.item, Some(ItemKind::Verdura));
    assert_eq!(took.amount, Some(3));
    assert_eq!(took.carriage, Some(serra));
    let totals = h.player_totals().unwrap();
    let took = totals.iter().find(|t| t.kind == "PlayerTook").unwrap();
    assert_eq!((took.times, took.units), (1, 3));
    if let Some(id) = hungry {
        assert_eq!(deeds[0].npc, Some(id));
        assert!(
            h.biography(id)
                .unwrap()
                .iter()
                .any(|e| e.kind == "PlayerGave")
        );
    }
}

#[test]
fn persists_on_disk_and_reads_concurrently() {
    let dir = TempDir::new();
    let (mut world, mut brain) = small_world(2);
    run_days(&mut world, &mut brain, 2);
    {
        let mut h = History::open(&dir.0).unwrap();
        h.sync(&world).unwrap();
        let mode: String = h
            .connection()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        // A reader sees what the writer committed, while it is open.
        let mut reader = History::open_read_only(&dir.0).unwrap();
        assert_eq!(reader.counts().unwrap().events, world.events_total());
        run_days(&mut world, &mut brain, 1);
        h.sync(&world).unwrap();
        assert_eq!(reader.counts().unwrap().events, world.events_total());
        reader.load_meta().unwrap();
        assert_eq!(reader.synced(), world.events_total());
    }
    // Reopening keeps everything and doesn't migrate again.
    let h = History::open(&dir.0).unwrap();
    assert_eq!(h.synced(), world.events_total());
    assert!(h.is_seeded());
    let versions: i64 = h
        .connection()
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, i64::from(SCHEMA_VERSION));
    assert!(dir.0.join(DB_FILE).exists());
}

#[test]
fn refuses_a_newer_schema() {
    let dir = TempDir::new();
    {
        let h = History::open(&dir.0).unwrap();
        h.connection()
            .execute("INSERT INTO schema_version VALUES (99, 0)", [])
            .unwrap();
    }
    assert!(matches!(History::open(&dir.0), Err(Error::NewerSchema(99))));
}

/// 20 game years with daily syncs: every birth, death and couple is there,
/// consistent with the world's own counters.
#[test]
fn twenty_years_headless_are_recorded_consistently() {
    let (mut world, mut brain) = small_world(11);
    let mut h = History::open_in_memory().unwrap();
    h.sync(&world).unwrap();
    let founders = world.npcs.len() as u64;
    let days = 20 * u64::from(world.params.days_per_year);
    for _ in 0..days {
        run_days(&mut world, &mut brain, 1);
        let r = h.sync(&world).unwrap();
        assert_eq!(r.gap, None);
    }
    let life = &world.life;
    assert!(life.births_total > 0 && life.deaths_total > 0, "{life:?}");
    let c = h.counts().unwrap();
    assert_eq!(c.events, world.events_total());
    assert_eq!(c.lost_events, 0);
    assert_eq!(h.count_kind("Born").unwrap(), life.births_total);
    assert_eq!(h.count_kind("NpcDied").unwrap(), life.deaths_total);
    assert_eq!(h.count_kind("Coupled").unwrap(), life.couples_formed_total);
    assert_eq!(c.people, founders + life.births_total);
    assert_eq!(c.alive, world.npcs.len() as u64);
    assert_eq!(c.dead, life.deaths_total);
    let causes = h.deaths_by_cause().unwrap();
    for (cause, n) in causes {
        assert_eq!(n, life.deaths_by_cause[cause.index()]);
    }
    let years = h.yearly_counts(world.params.days_per_year).unwrap();
    assert_eq!(
        years.iter().map(|y| y.births).sum::<u64>(),
        life.births_total
    );
    assert_eq!(
        years.iter().map(|y| y.deaths).sum::<u64>(),
        life.deaths_total
    );
    let snapshots = h.yearly_snapshots().unwrap();
    assert_eq!(snapshots.len(), 21);
    assert_eq!(snapshots[0].population as u64, founders);

    // Every living NPC is in the history, alive, with the same name.
    for npc in &world.npcs {
        let p = h.person(npc.id).unwrap().unwrap();
        assert!(p.is_alive());
        assert_eq!(p.name, npc.name);
        assert_eq!(p.born, Some(npc.born));
        assert_eq!(
            p.age_at(world.clock, world.params.days_per_year),
            Some(npc.age_years(world.clock, world.params.days_per_year))
        );
    }
    // Children born on the train know both parents; the dead stay.
    let child = world.npcs.iter().find(|n| !world.is_founder(n.id)).unwrap();
    let p = h.person(child.id).unwrap().unwrap();
    assert!(p.mother.is_some() && p.father.is_some());
    let oldest = h.longest_lived(5).unwrap();
    assert!(!oldest.is_empty() && oldest.iter().all(|p| !p.is_alive()));
    assert!(world.npc(oldest[0].id).is_none());
    let bio = h.biography(oldest[0].id).unwrap();
    assert!(bio.iter().any(|e| e.kind == "NpcDied"));
}
