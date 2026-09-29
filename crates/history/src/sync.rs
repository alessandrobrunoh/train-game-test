//! Scrittura: cattura degli eventi dal mondo, inserimento, rollback.

use rusqlite::{Connection, params};
use sim::{
    DeathCause, DeliberationKind, Event, EventKind, ItemKind, NpcId, RelationKind, Sex, World,
};

use crate::{History, Result, set_meta};

/// Everything a sync writes, captured from the world (owned: it can be sent
/// to a writer thread). See [`SyncBatch::capture`].
#[derive(Clone, Debug, PartialEq)]
pub struct SyncBatch {
    /// `seq` of `events[0]`.
    pub first_seq: u64,
    /// New events, oldest first.
    pub events: Vec<Event>,
    /// Events `[from, to)` that were dropped from memory before being synced.
    pub gap: Option<(u64, u64)>,
    /// [`World::events_total`] when captured (= `first_seq + events.len()`).
    pub world_total: u64,
    /// Game time when captured (minutes).
    pub time: u64,
    /// NPC ids below this are founders (see [`World::is_founder`]).
    pub founders: u32,
    /// Living people and couples, to seed the `people` / `couples` tables
    /// (only in the first batch of a history).
    pub seed: Option<Seed>,
    /// Population snapshot, stored once per game year.
    pub year: YearSnapshot,
}

/// Living NPCs at seeding time.
#[derive(Clone, Debug, PartialEq)]
pub struct Seed {
    pub people: Vec<SeedPerson>,
    /// Living couples `(a, b)` with `a < b`.
    pub couples: Vec<(NpcId, NpcId)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeedPerson {
    pub id: NpcId,
    pub name: String,
    pub sex: Sex,
    pub born: i64,
    pub founder: bool,
    pub mother: Option<NpcId>,
    pub father: Option<NpcId>,
}

/// Population at the start of a game year (well, at the first sync in it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YearSnapshot {
    /// Game year, from 1.
    pub year: i64,
    pub population: u32,
    pub founders: u32,
    pub couples: u32,
    pub avg_age: f32,
}

/// What a sync did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncReport {
    /// Events appended.
    pub inserted: u64,
    /// Events lost before this sync (`[from, to)`), already recorded in `gaps`.
    pub gap: Option<(u64, u64)>,
    /// The world was behind the history: rolled back to this seq first.
    pub rolled_back_to: Option<u64>,
    /// People were seeded from the world by this sync.
    pub seeded: bool,
}

/// What [`History::reconcile`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// The history was rolled back to this seq (it had events the world never had).
    pub rolled_back_to: Option<u64>,
    /// The oldest event still in memory already differs from the history:
    /// older ones may differ too but can't be checked (another branch of the
    /// same run was synced). The history keeps them.
    pub unverifiable: bool,
}

impl SyncBatch {
    /// Captures the events with `seq >= next` still in memory (`next` is the
    /// history's [`History::synced`]); `seed` also snapshots the living people.
    ///
    /// The world keeps at least `max_events - max_events / 4` events (1500
    /// by default) after a trim: capture at least that often, in events.
    pub fn capture(world: &World, next: u64, seed: bool) -> SyncBatch {
        let total = world.events_total();
        let in_memory = world.events.len() as u64;
        let oldest = total - in_memory;
        let from = next.clamp(oldest, total);
        let gap = (next < oldest).then_some((next, oldest));
        let events = world.events[(from - oldest) as usize..].to_vec();
        SyncBatch {
            first_seq: from,
            events,
            gap,
            world_total: total,
            time: world.clock.0,
            founders: founders(world),
            seed: seed.then(|| Seed::of(world)),
            year: YearSnapshot::of(world),
        }
    }
}

impl Seed {
    pub fn of(world: &World) -> Seed {
        let mut people = Vec::with_capacity(world.npcs.len());
        let mut couples = Vec::new();
        for npc in &world.npcs {
            let mut mother = None;
            let mut father = None;
            for parent in npc.relations_of(RelationKind::Parent) {
                match world.npc(parent.other).map(|p| p.sex) {
                    Some(Sex::Female) => mother = Some(parent.other),
                    Some(Sex::Male) => father = Some(parent.other),
                    None => {}
                }
            }
            if let Some(partner) = npc.partner()
                && npc.id < partner
            {
                couples.push((npc.id, partner));
            }
            people.push(SeedPerson {
                id: npc.id,
                name: npc.name.clone(),
                sex: npc.sex,
                born: npc.born,
                founder: world.is_founder(npc.id),
                mother,
                father,
            });
        }
        Seed { people, couples }
    }
}

impl YearSnapshot {
    pub fn of(world: &World) -> YearSnapshot {
        let mut founders = 0;
        let mut partnered = 0;
        let mut ages = 0u64;
        for npc in &world.npcs {
            founders += u32::from(world.is_founder(npc.id));
            partnered += u32::from(npc.partner().is_some());
            ages += u64::from(npc.age);
        }
        let population = world.npcs.len() as u32;
        YearSnapshot {
            year: year_of(world.clock.0 as i64, world.params.days_per_year),
            population,
            founders,
            couples: partnered / 2,
            avg_age: ages as f32 / population.max(1) as f32,
        }
    }
}

/// Number of founders: ids below it are founders ([`World::is_founder`] is monotonic).
fn founders(world: &World) -> u32 {
    let (mut lo, mut hi) = (0u32, world.next_npc_id().0);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if world.is_founder(NpcId(mid)) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Game year (from 1; 0 or less before the departure) of a time in minutes.
pub(crate) fn year_of(minutes: i64, days_per_year: u32) -> i64 {
    let year = i64::from(days_per_year.max(1)) * sim::MINUTES_PER_DAY as i64;
    minutes.div_euclid(year) + 1
}

/// Machine-friendly columns of an event (see the schema in the crate docs).
pub(crate) struct Columns {
    pub kind: &'static str,
    pub npc: Option<NpcId>,
    pub other: Option<NpcId>,
    /// Involved NPCs beyond `npc` and `other` (a newborn's father).
    pub third: Option<NpcId>,
    pub carriage: Option<u16>,
    pub item: Option<ItemKind>,
    pub amount: Option<u32>,
    pub price: Option<u32>,
}

pub(crate) fn columns(kind: &EventKind) -> Columns {
    let mut c = Columns {
        kind: kind_name(kind),
        npc: None,
        other: None,
        third: None,
        carriage: None,
        item: None,
        amount: None,
        price: None,
    };
    match *kind {
        EventKind::NpcStarving { npc, .. }
        | EventKind::CameOfAge { npc, .. }
        | EventKind::Retired { npc, .. } => c.npc = Some(npc),
        EventKind::NpcDied { npc, age, .. } => {
            c.npc = Some(npc);
            c.amount = Some(age);
        }
        EventKind::Born {
            npc,
            mother,
            father,
            ..
        } => {
            c.npc = Some(npc);
            c.other = Some(mother);
            c.third = Some(father);
        }
        EventKind::Coupled {
            npc,
            partner,
            moved_to,
            ..
        } => {
            c.npc = Some(npc);
            c.other = Some(partner);
            c.carriage = moved_to.map(|m| m.0);
        }
        EventKind::Widowed { npc, partner, .. } => {
            c.npc = Some(npc);
            c.other = Some(partner);
        }
        EventKind::BirthDenied { mother, father, .. } => {
            c.npc = Some(mother);
            c.other = Some(father);
        }
        EventKind::Shortage { item } | EventKind::Restocked { item } => c.item = Some(item),
        EventKind::ItemBought {
            npc,
            item,
            price,
            carriage,
            ..
        } => {
            c.npc = Some(npc);
            c.item = Some(item);
            c.amount = Some(1);
            c.price = Some(price);
            c.carriage = Some(carriage.0);
        }
        EventKind::ItemBroke { npc, item, .. } => {
            c.npc = Some(npc);
            c.item = Some(item);
        }
        EventKind::PlayerTook {
            item,
            amount,
            carriage,
        } => {
            c.item = Some(item);
            c.amount = Some(amount);
            c.carriage = Some(carriage.0);
        }
        EventKind::PlayerBought {
            item,
            price,
            carriage,
        } => {
            c.item = Some(item);
            c.amount = Some(1);
            c.price = Some(price);
            c.carriage = Some(carriage.0);
        }
        EventKind::PlayerGave { npc, item, .. } => {
            c.npc = Some(npc);
            c.item = Some(item);
            c.amount = Some(1);
        }
        EventKind::DeliberationAsked { npc, kind, .. }
        | EventKind::DeliberationResolved { npc, kind, .. } => {
            c.npc = Some(npc);
            c.other = kind.other();
            if let DeliberationKind::Theft { item, market, .. } = kind {
                c.item = Some(item);
                c.carriage = Some(market.0);
            }
        }
        EventKind::Theft {
            npc,
            item,
            carriage,
            fine,
            ..
        } => {
            c.npc = Some(npc);
            c.item = Some(item);
            c.carriage = Some(carriage.0);
            c.price = Some(fine);
        }
        EventKind::HelpAsked {
            npc,
            helper,
            tokens,
            ..
        } => {
            c.npc = Some(npc);
            c.other = Some(helper);
            c.amount = Some(tokens);
        }
        EventKind::ProtestCalled { place, .. } => c.carriage = Some(place.0),
        EventKind::AdminConceded { protesters, .. } => c.amount = Some(protesters),
    }
    c
}

/// Stable name of an event kind (the variant name), stored in `events.kind`.
pub fn kind_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::NpcStarving { .. } => "NpcStarving",
        EventKind::NpcDied { .. } => "NpcDied",
        EventKind::Born { .. } => "Born",
        EventKind::CameOfAge { .. } => "CameOfAge",
        EventKind::Retired { .. } => "Retired",
        EventKind::Coupled { .. } => "Coupled",
        EventKind::Widowed { .. } => "Widowed",
        EventKind::BirthDenied { .. } => "BirthDenied",
        EventKind::Shortage { .. } => "Shortage",
        EventKind::Restocked { .. } => "Restocked",
        EventKind::ItemBought { .. } => "ItemBought",
        EventKind::ItemBroke { .. } => "ItemBroke",
        EventKind::PlayerTook { .. } => "PlayerTook",
        EventKind::PlayerBought { .. } => "PlayerBought",
        EventKind::PlayerGave { .. } => "PlayerGave",
        EventKind::DeliberationAsked { .. } => "DeliberationAsked",
        EventKind::DeliberationResolved { .. } => "DeliberationResolved",
        EventKind::Theft { .. } => "Theft",
        EventKind::HelpAsked { .. } => "HelpAsked",
        EventKind::ProtestCalled { .. } => "ProtestCalled",
        EventKind::AdminConceded { .. } => "AdminConceded",
    }
}

/// Stored name of an item (the variant name).
pub(crate) fn item_name(item: ItemKind) -> &'static str {
    match item {
        ItemKind::Verdura => "Verdura",
        ItemKind::Razione => "Razione",
        ItemKind::Rottame => "Rottame",
        ItemKind::Attrezzo => "Attrezzo",
        ItemKind::Vestito => "Vestito",
    }
}

pub(crate) fn cause_name(cause: DeathCause) -> &'static str {
    match cause {
        DeathCause::Starvation => "Starvation",
        DeathCause::OldAge => "OldAge",
    }
}

pub(crate) fn sex_code(sex: Sex) -> &'static str {
    match sex {
        Sex::Female => "F",
        Sex::Male => "M",
    }
}

/// Italian message of an event without the leading "[Giorno N hh:mm] ".
pub fn event_text(event: &Event) -> String {
    let text = event.to_string();
    match text.split_once("] ") {
        Some((_, message)) => message.to_string(),
        None => text,
    }
}

/// JSON stored in `events.data`.
pub(crate) fn event_json(kind: &EventKind) -> String {
    serde_json::to_string(kind).unwrap_or_default()
}

impl History {
    /// Appends the world's new events (and seeds the people at the first
    /// sync) in one transaction. Idempotent: syncing again without new events
    /// does nothing. If the world is behind the history (an older save was
    /// loaded) the history is [reconciled](History::reconcile) first.
    pub fn sync(&mut self, world: &World) -> Result<SyncReport> {
        let mut rolled_back_to = None;
        if world.events_total() < self.synced {
            rolled_back_to = self.reconcile(world)?.rolled_back_to;
        }
        let batch = SyncBatch::capture(world, self.synced, !self.is_seeded());
        let mut report = self.apply(&batch)?;
        report.rolled_back_to = report.rolled_back_to.or(rolled_back_to);
        Ok(report)
    }

    /// Writes a captured batch in one transaction. Events already stored
    /// (`seq < synced`) are skipped, so applying the same batch twice is harmless.
    pub fn apply(&mut self, batch: &SyncBatch) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let tx = self.conn.unchecked_transaction()?;
        let mut synced = self.synced;
        let mut seeded_at = self.seeded_at;
        if batch.world_total < synced {
            // The world went back in time: forget what never happened.
            let reset = rollback(&tx, batch.world_total, seeded_at)?;
            if reset {
                seeded_at = None;
            }
            synced = synced.min(batch.world_total);
            report.rolled_back_to = Some(batch.world_total);
        }
        if seeded_at.is_none()
            && let Some(seed) = &batch.seed
        {
            insert_seed(&tx, seed)?;
            // Effects of the events from here on are recorded.
            let first = synced.max(batch.first_seq);
            seeded_at = Some(first);
            set_meta(&tx, "seeded_at", first)?;
            report.seeded = true;
        }
        if let Some((from, to)) = batch.gap
            && to > synced
        {
            let from = from.max(synced);
            tx.prepare_cached(
                "INSERT OR REPLACE INTO gaps (from_seq, to_seq, time) VALUES (?1, ?2, ?3)",
            )?
            .execute(params![from as i64, to as i64, batch.time as i64])?;
            report.gap = Some((from, to));
            synced = to;
        }
        for (k, event) in batch.events.iter().enumerate() {
            let seq = batch.first_seq + k as u64;
            if seq < synced {
                continue;
            }
            insert_event(&tx, seq, event, batch.founders)?;
            report.inserted += 1;
            synced = seq + 1;
        }
        synced = synced.max(batch.world_total);
        tx.prepare_cached(
            "INSERT OR IGNORE INTO yearly (year, time, seq, population, founders, couples, avg_age)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?
        .execute(params![
            batch.year.year,
            batch.time as i64,
            batch.world_total as i64,
            batch.year.population,
            batch.year.founders,
            batch.year.couples,
            f64::from(batch.year.avg_age),
        ])?;
        set_meta(&tx, "synced", synced)?;
        tx.commit()?;
        self.synced = synced;
        self.seeded_at = seeded_at;
        Ok(report)
    }

    /// Deletes everything recorded from event `seq` on (events, births,
    /// deaths, couples, snapshots), as if the history had been synced up to
    /// `seq` only: use it when loading a save whose `events_total()` is `seq`.
    /// If `seq` is before the point where people were seeded, the whole
    /// history is cleared (the next sync seeds again from the loaded world).
    pub fn rollback_to(&mut self, seq: u64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let reset = rollback(&tx, seq, self.seeded_at)?;
        let synced = if reset { 0 } else { self.synced.min(seq) };
        set_meta(&tx, "synced", synced)?;
        tx.commit()?;
        self.synced = synced;
        if reset {
            self.seeded_at = None;
        }
        Ok(())
    }

    /// Makes the history agree with a (just loaded) world: rolls back what
    /// the world doesn't have yet, then compares the events still in memory
    /// with the stored ones and rolls back from the first difference (the
    /// history came from another branch of the same run).
    pub fn reconcile(&mut self, world: &World) -> Result<ReconcileReport> {
        let mut report = ReconcileReport::default();
        let total = world.events_total();
        if total < self.synced {
            self.rollback_to(total)?;
            report.rolled_back_to = Some(total);
        }
        let oldest = total - world.events.len() as u64;
        let end = self.synced.min(total);
        if end <= oldest {
            return Ok(report);
        }
        let mismatch = {
            let mut stmt = self.conn.prepare(
                "SELECT seq, time, data FROM events WHERE seq >= ?1 AND seq < ?2 ORDER BY seq",
            )?;
            let mut rows = stmt.query(params![oldest as i64, end as i64])?;
            let mut expected = oldest;
            let mut mismatch = None;
            while let Some(row) = rows.next()? {
                let seq = row.get::<_, i64>(0)? as u64;
                let event = &world.events[(seq - oldest) as usize];
                let same = seq == expected
                    && row.get::<_, i64>(1)? as u64 == event.time.0
                    && row.get_ref(2)?.as_str().ok() == Some(event_json(&event.kind).as_str());
                if !same {
                    // A hole in the stored events (a gap) counts as a difference.
                    mismatch = Some(expected.min(seq));
                    break;
                }
                expected = seq + 1;
            }
            mismatch.or((expected < end).then_some(expected))
        };
        if let Some(seq) = mismatch {
            // Missing rows inside a recorded gap are fine: only real differences matter.
            let in_gap: bool = self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM gaps WHERE from_seq <= ?1 AND to_seq > ?1)",
                [seq as i64],
                |r| r.get(0),
            )?;
            if !in_gap {
                self.rollback_to(seq)?;
                report.rolled_back_to = Some(seq);
                report.unverifiable = seq == oldest && oldest > 0;
            }
        }
        Ok(report)
    }
}

/// Deletes what happened from `seq` on. Returns true if everything was
/// cleared because `seq` precedes the seeding.
fn rollback(conn: &Connection, seq: u64, seeded_at: Option<u64>) -> Result<bool> {
    if seeded_at.is_some_and(|at| seq < at) {
        conn.execute_batch(
            "DELETE FROM events; DELETE FROM event_npcs; DELETE FROM people; DELETE FROM couples;
             DELETE FROM yearly; DELETE FROM gaps; DELETE FROM meta;",
        )?;
        return Ok(true);
    }
    let s = seq as i64;
    conn.execute("DELETE FROM events WHERE seq >= ?1", [s])?;
    conn.execute("DELETE FROM event_npcs WHERE seq >= ?1", [s])?;
    conn.execute("DELETE FROM people WHERE born_seq >= ?1", [s])?;
    conn.execute(
        "UPDATE people SET died = NULL, death_cause = NULL, death_age = NULL, died_seq = NULL
         WHERE died_seq >= ?1",
        [s],
    )?;
    conn.execute("DELETE FROM couples WHERE formed_seq >= ?1", [s])?;
    conn.execute(
        "UPDATE couples SET ended = NULL, ended_seq = NULL WHERE ended_seq >= ?1",
        [s],
    )?;
    conn.execute("DELETE FROM yearly WHERE seq > ?1", [s])?;
    conn.execute("DELETE FROM gaps WHERE from_seq >= ?1", [s])?;
    conn.execute("UPDATE gaps SET to_seq = ?1 WHERE to_seq > ?1", [s])?;
    Ok(false)
}

fn insert_seed(conn: &Connection, seed: &Seed) -> Result<()> {
    let mut person = conn.prepare_cached(
        "INSERT INTO people (id, name, sex, born, founder, mother, father)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET name = excluded.name, sex = excluded.sex,
             born = excluded.born, founder = excluded.founder,
             mother = COALESCE(people.mother, excluded.mother),
             father = COALESCE(people.father, excluded.father)",
    )?;
    for p in &seed.people {
        person.execute(params![
            p.id.0,
            p.name,
            sex_code(p.sex),
            p.born,
            p.founder,
            p.mother.map(|m| m.0),
            p.father.map(|f| f.0),
        ])?;
    }
    let mut couple = conn.prepare_cached("INSERT OR IGNORE INTO couples (a, b) VALUES (?1, ?2)")?;
    for &(a, b) in &seed.couples {
        couple.execute(params![a.0, b.0])?;
    }
    Ok(())
}

fn insert_event(conn: &Connection, seq: u64, event: &Event, founders: u32) -> Result<()> {
    let c = columns(&event.kind);
    let s = seq as i64;
    let time = event.time.0 as i64;
    conn.prepare_cached(
        "INSERT OR REPLACE INTO events (seq, time, kind, npc, other, carriage, item, amount, price, text, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        s,
        time,
        c.kind,
        c.npc.map(|n| n.0),
        c.other.map(|n| n.0),
        c.carriage,
        c.item.map(item_name),
        c.amount,
        c.price,
        event_text(event),
        event_json(&event.kind),
    ])?;
    let mut involved =
        conn.prepare_cached("INSERT OR IGNORE INTO event_npcs (npc, seq) VALUES (?1, ?2)")?;
    for npc in [c.npc, c.other, c.third].into_iter().flatten() {
        involved.execute(params![npc.0, s])?;
    }

    match &event.kind {
        EventKind::Born {
            npc,
            name,
            sex,
            mother,
            father,
            mother_name,
            father_name,
        } => {
            // Parents unknown to the history (dropped before the seeding) get a stub.
            let mut stub = conn.prepare_cached(
                "INSERT OR IGNORE INTO people (id, name, sex, founder) VALUES (?1, ?2, ?3, ?4)",
            )?;
            stub.execute(params![mother.0, mother_name, "F", mother.0 < founders])?;
            stub.execute(params![father.0, father_name, "M", father.0 < founders])?;
            conn.prepare_cached(
                "INSERT INTO people (id, name, sex, born, founder, mother, father, born_seq)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)
                 ON CONFLICT (id) DO UPDATE SET born = excluded.born, mother = excluded.mother,
                     father = excluded.father, born_seq = excluded.born_seq",
            )?
            .execute(params![
                npc.0,
                name,
                sex_code(*sex),
                time,
                mother.0,
                father.0,
                s
            ])?;
        }
        EventKind::NpcDied {
            npc,
            name,
            cause,
            age,
            sex,
        } => {
            let updated = conn
                .prepare_cached(
                    "UPDATE people SET died = ?2, death_cause = ?3, death_age = ?4, died_seq = ?5
                     WHERE id = ?1",
                )?
                .execute(params![npc.0, time, cause_name(*cause), age, s])?;
            if updated == 0 {
                conn.prepare_cached(
                    "INSERT INTO people (id, name, sex, died, death_cause, death_age, died_seq, founder)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?
                .execute(params![
                    npc.0,
                    name,
                    sex_code(*sex),
                    time,
                    cause_name(*cause),
                    age,
                    s,
                    npc.0 < founders
                ])?;
            }
            conn.prepare_cached(
                "UPDATE couples SET ended = ?2, ended_seq = ?3
                 WHERE (a = ?1 OR b = ?1) AND ended IS NULL",
            )?
            .execute(params![npc.0, time, s])?;
        }
        EventKind::Coupled { npc, partner, .. } => {
            let (a, b) = if npc < partner {
                (npc, partner)
            } else {
                (partner, npc)
            };
            conn.prepare_cached(
                "INSERT INTO couples (a, b, formed, formed_seq) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (a, b) DO UPDATE SET formed = excluded.formed,
                     formed_seq = excluded.formed_seq",
            )?
            .execute(params![a.0, b.0, time, s])?;
        }
        _ => {}
    }
    Ok(())
}
