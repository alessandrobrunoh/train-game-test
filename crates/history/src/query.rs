//! Lettura: biografie, alberi genealogici, ricerche, registro del giocatore, record.
//!
//! All queries are indexed (or scan small aggregates) and meant to run in
//! well under a millisecond on a history of a few hundred thousand events.

use rusqlite::{OptionalExtension, Row, params};
use sim::{CarriageId, DeathCause, EventKind, GameTime, ItemKind, NpcId, Sex};

use crate::sync::year_of;
use crate::{History, Result};

/// A stored event.
#[derive(Clone, Debug, PartialEq)]
pub struct EventRow {
    /// Global index (position in `World::events_total` order).
    pub seq: u64,
    pub time: GameTime,
    /// Variant name of the [`EventKind`], e.g. `"Born"`.
    pub kind: String,
    pub npc: Option<NpcId>,
    pub other: Option<NpcId>,
    pub carriage: Option<CarriageId>,
    pub item: Option<ItemKind>,
    pub amount: Option<u32>,
    pub price: Option<u32>,
    /// Italian message, without the time.
    pub text: String,
    /// JSON of the [`EventKind`].
    pub data: String,
}

impl EventRow {
    /// The original event, decoded from `data`.
    pub fn event_kind(&self) -> Option<EventKind> {
        serde_json::from_str(&self.data).ok()
    }
}

/// Someone who lived on the train (alive or dead).
#[derive(Clone, Debug, PartialEq)]
pub struct PersonRow {
    pub id: NpcId,
    pub name: String,
    pub sex: Sex,
    /// Birth time in game minutes (negative before the departure); `None`
    /// if unknown (seen only in a death or as a parent after a gap).
    pub born: Option<i64>,
    pub died: Option<GameTime>,
    pub death_cause: Option<DeathCause>,
    /// Whole years at death.
    pub death_age: Option<u32>,
    pub mother: Option<NpcId>,
    pub father: Option<NpcId>,
    pub founder: bool,
}

impl PersonRow {
    pub fn is_alive(&self) -> bool {
        self.died.is_none()
    }

    /// Age in whole years at `now` (or at death), if the birth is known.
    pub fn age_at(&self, now: GameTime, days_per_year: u32) -> Option<u32> {
        if let Some(age) = self.death_age {
            return Some(age);
        }
        let born = self.born?;
        let end = self.died.unwrap_or(now).0 as i64;
        let year = i64::from(days_per_year.max(1)) * sim::MINUTES_PER_DAY as i64;
        Some(((end - born).max(0) / year) as u32)
    }

    /// Everything after the first space of the name.
    pub fn surname(&self) -> &str {
        self.name
            .split_once(' ')
            .map_or(self.name.as_str(), |(_, last)| last)
    }
}

/// A couple, `a < b`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoupleRow {
    pub a: NpcId,
    pub b: NpcId,
    /// `None`: already a couple when the history started.
    pub formed: Option<GameTime>,
    /// When one of the two died.
    pub ended: Option<GameTime>,
}

/// A person in a family tree. Ancestor nodes only have `parents`,
/// descendant nodes only `children` (and `partners`); the root has both.
#[derive(Clone, Debug, PartialEq)]
pub struct FamilyNode {
    pub person: PersonRow,
    /// Partners (in order of coupling), for the root and its descendants.
    pub partners: Vec<PersonRow>,
    /// Mother first, then father (those known).
    pub parents: Vec<FamilyNode>,
    /// Oldest first.
    pub children: Vec<FamilyNode>,
}

pub type FamilyTree = FamilyNode;

impl FamilyNode {
    /// People in the tree, the root included.
    pub fn len(&self) -> usize {
        1 + self.parents.iter().map(FamilyNode::len).sum::<usize>()
            + self.children.iter().map(FamilyNode::len).sum::<usize>()
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

/// Births and deaths in a game year.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct YearCounts {
    pub year: i64,
    pub births: u64,
    pub deaths: u64,
}

/// Row of the `yearly` table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YearSnapshotRow {
    pub year: i64,
    pub time: GameTime,
    pub population: u32,
    pub founders: u32,
    pub couples: u32,
    pub avg_age: f32,
}

/// What the player did, summed by kind and item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerTotal {
    /// `"PlayerTook"`, `"PlayerBought"` or `"PlayerGave"`.
    pub kind: String,
    pub item: Option<ItemKind>,
    /// Number of events.
    pub times: u64,
    /// Units of the item.
    pub units: u64,
    /// Tokens spent.
    pub tokens: u64,
}

/// Size of the history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryCounts {
    pub events: u64,
    pub people: u64,
    pub alive: u64,
    pub dead: u64,
    pub couples: u64,
    /// Events lost (dropped from memory before a sync), and in how many gaps.
    pub lost_events: u64,
    pub gaps: u64,
}

const PERSON_COLS: &str =
    "id, name, sex, born, died, death_cause, death_age, mother, father, founder";
const EVENT_COLS: &str = "seq, time, kind, npc, other, carriage, item, amount, price, text, data";
/// Event kinds caused by the player.
const PLAYER_KINDS: &str = "('PlayerTook', 'PlayerBought', 'PlayerGave')";

fn person_row(r: &Row) -> rusqlite::Result<PersonRow> {
    Ok(PersonRow {
        id: NpcId(r.get(0)?),
        name: r.get(1)?,
        sex: if r.get_ref(2)?.as_str()? == "F" {
            Sex::Female
        } else {
            Sex::Male
        },
        born: r.get(3)?,
        died: r
            .get::<_, Option<i64>>(4)?
            .map(|t| GameTime(t.max(0) as u64)),
        death_cause: r.get::<_, Option<String>>(5)?.and_then(|c| parse_cause(&c)),
        death_age: r.get(6)?,
        mother: r.get::<_, Option<u32>>(7)?.map(NpcId),
        father: r.get::<_, Option<u32>>(8)?.map(NpcId),
        founder: r.get(9)?,
    })
}

fn event_row(r: &Row) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        seq: r.get::<_, i64>(0)? as u64,
        time: GameTime(r.get::<_, i64>(1)?.max(0) as u64),
        kind: r.get(2)?,
        npc: r.get::<_, Option<u32>>(3)?.map(NpcId),
        other: r.get::<_, Option<u32>>(4)?.map(NpcId),
        carriage: r.get::<_, Option<u16>>(5)?.map(CarriageId),
        item: r.get::<_, Option<String>>(6)?.and_then(|i| parse_item(&i)),
        amount: r.get(7)?,
        price: r.get(8)?,
        text: r.get(9)?,
        data: r.get(10)?,
    })
}

fn parse_cause(name: &str) -> Option<DeathCause> {
    DeathCause::ALL
        .into_iter()
        .find(|&c| crate::sync::cause_name(c) == name)
}

pub(crate) fn parse_item(name: &str) -> Option<ItemKind> {
    ItemKind::ALL
        .into_iter()
        .find(|&i| crate::sync::item_name(i) == name)
}

impl History {
    /// Someone who ever lived on the train (since the history started).
    pub fn person(&self, id: NpcId) -> Result<Option<PersonRow>> {
        let sql = format!("SELECT {PERSON_COLS} FROM people WHERE id = ?1");
        Ok(self
            .conn
            .prepare_cached(&sql)?
            .query_row([id.0], person_row)
            .optional()?)
    }

    /// People whose name contains `text` (case-insensitive for ASCII),
    /// the living first, then by name.
    pub fn search_people(&self, text: &str, limit: usize) -> Result<Vec<PersonRow>> {
        let escaped: String = text
            .trim()
            .chars()
            .flat_map(|c| match c {
                '%' | '_' | '\\' => vec!['\\', c],
                c => vec![c],
            })
            .collect();
        let sql = format!(
            "SELECT {PERSON_COLS} FROM people WHERE name LIKE '%' || ?1 || '%' ESCAPE '\\'
             ORDER BY died IS NOT NULL, name, id LIMIT ?2"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![escaped, limit as i64], person_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every event involving `npc` (as main NPC, partner, parent of a
    /// newborn, ...), oldest first.
    pub fn biography(&self, npc: NpcId) -> Result<Vec<EventRow>> {
        let sql = format!(
            "SELECT {} FROM event_npcs x JOIN events e ON e.seq = x.seq
             WHERE x.npc = ?1 ORDER BY x.seq",
            EVENT_COLS
                .split(", ")
                .map(|c| format!("e.{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([npc.0], event_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Children of `id`, oldest first.
    pub fn children(&self, id: NpcId) -> Result<Vec<PersonRow>> {
        let sql = format!(
            "SELECT {PERSON_COLS} FROM people WHERE mother = ?1
             UNION
             SELECT {PERSON_COLS} FROM people WHERE father = ?1
             ORDER BY born, id"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([id.0], person_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Couples `id` was part of, in order of coupling.
    pub fn couples_of(&self, id: NpcId) -> Result<Vec<CoupleRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a, b, formed, ended FROM (
                 SELECT a, b, formed, ended FROM couples WHERE a = ?1
                 UNION ALL
                 SELECT a, b, formed, ended FROM couples WHERE b = ?1
             ) ORDER BY formed IS NOT NULL, formed",
        )?;
        let time = |t: Option<i64>| t.map(|t| GameTime(t.max(0) as u64));
        let rows = stmt.query_map([id.0], |r| {
            Ok(CoupleRow {
                a: NpcId(r.get(0)?),
                b: NpcId(r.get(1)?),
                formed: time(r.get(2)?),
                ended: time(r.get(3)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Partners of `id`, in order of coupling.
    pub fn partners(&self, id: NpcId) -> Result<Vec<PersonRow>> {
        let mut partners = Vec::new();
        for couple in self.couples_of(id)? {
            let other = if couple.a == id { couple.b } else { couple.a };
            if let Some(p) = self.person(other)? {
                partners.push(p);
            }
        }
        Ok(partners)
    }

    /// `npc` with its ancestors (`up` generations: 1 = parents) and
    /// descendants (`down` generations: 1 = children). `None` if unknown.
    pub fn family_tree(&self, npc: NpcId, up: u32, down: u32) -> Result<Option<FamilyTree>> {
        let Some(person) = self.person(npc)? else {
            return Ok(None);
        };
        let mut root = self.descendants(person, down)?;
        root.parents = self.ancestors(&root.person, up)?;
        Ok(Some(root))
    }

    fn ancestors(&self, person: &PersonRow, up: u32) -> Result<Vec<FamilyNode>> {
        if up == 0 {
            return Ok(Vec::new());
        }
        let mut parents = Vec::new();
        for id in [person.mother, person.father].into_iter().flatten() {
            if let Some(parent) = self.person(id)? {
                let grandparents = self.ancestors(&parent, up - 1)?;
                parents.push(FamilyNode {
                    person: parent,
                    partners: Vec::new(),
                    parents: grandparents,
                    children: Vec::new(),
                });
            }
        }
        Ok(parents)
    }

    fn descendants(&self, person: PersonRow, down: u32) -> Result<FamilyNode> {
        let partners = self.partners(person.id)?;
        let mut children = Vec::new();
        if down > 0 {
            for child in self.children(person.id)? {
                children.push(self.descendants(child, down - 1)?);
            }
        }
        Ok(FamilyNode {
            person,
            partners,
            parents: Vec::new(),
            children,
        })
    }

    /// The player's most recent deeds (took, bought, gave), newest first.
    pub fn player_actions(&self, limit: usize) -> Result<Vec<EventRow>> {
        let sql = format!(
            "SELECT {EVENT_COLS} FROM events WHERE kind IN {PLAYER_KINDS}
             ORDER BY seq DESC LIMIT ?1"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([limit as i64], event_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The player's deeds summed by kind and item.
    pub fn player_totals(&self) -> Result<Vec<PlayerTotal>> {
        let sql = format!(
            "SELECT kind, item, COUNT(*), TOTAL(amount), TOTAL(price) FROM events
             WHERE kind IN {PLAYER_KINDS} GROUP BY kind, item ORDER BY kind, item"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(PlayerTotal {
                kind: r.get(0)?,
                item: r.get::<_, Option<String>>(1)?.and_then(|i| parse_item(&i)),
                times: r.get::<_, i64>(2)? as u64,
                units: r.get::<_, f64>(3)? as u64,
                tokens: r.get::<_, f64>(4)? as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// People with the most children, with the count.
    pub fn most_children(&self, limit: usize) -> Result<Vec<(PersonRow, u32)>> {
        let sql = format!(
            "SELECT {}, n FROM (
                 SELECT parent, COUNT(*) AS n FROM (
                     SELECT mother AS parent FROM people WHERE mother IS NOT NULL
                     UNION ALL
                     SELECT father FROM people WHERE father IS NOT NULL
                 ) GROUP BY parent
             ) JOIN people p ON p.id = parent
             ORDER BY n DESC, p.id LIMIT ?1",
            prefixed("p")
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([limit as i64], |r| Ok((person_row(r)?, r.get(10)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The dead who lived longest.
    pub fn longest_lived(&self, limit: usize) -> Result<Vec<PersonRow>> {
        let sql = format!(
            "SELECT {PERSON_COLS} FROM people WHERE death_age IS NOT NULL
             ORDER BY death_age DESC, died LIMIT ?1"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([limit as i64], person_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Surnames carried by the most people ever: `(surname, total, alive)`.
    pub fn largest_families(&self, limit: usize) -> Result<Vec<(String, u32, u32)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT substr(name, instr(name, ' ') + 1) AS surname, COUNT(*) AS n,
                    SUM(died IS NULL)
             FROM people GROUP BY surname ORDER BY n DESC, surname LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Deaths recorded per cause (every cause listed, also with 0).
    pub fn deaths_by_cause(&self) -> Result<Vec<(DeathCause, u64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT death_cause, COUNT(*) FROM people WHERE died IS NOT NULL GROUP BY death_cause",
        )?;
        let mut counts: Vec<(DeathCause, u64)> = DeathCause::ALL.map(|c| (c, 0)).to_vec();
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (cause, n) = row?;
            if let Some(cause) = cause.as_deref().and_then(parse_cause) {
                counts[cause.index()].1 += n as u64;
            }
        }
        Ok(counts)
    }

    /// Births and deaths per game year (years with neither are left out),
    /// counted from the stored events.
    pub fn yearly_counts(&self, days_per_year: u32) -> Result<Vec<YearCounts>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT kind, time FROM events WHERE kind = 'Born'
             UNION ALL
             SELECT kind, time FROM events WHERE kind = 'NpcDied'",
        )?;
        let mut years: Vec<YearCounts> = Vec::new();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (kind, time) = row?;
            let year = year_of(time, days_per_year);
            let entry = match years.binary_search_by_key(&year, |y| y.year) {
                Ok(i) => &mut years[i],
                Err(i) => {
                    years.insert(
                        i,
                        YearCounts {
                            year,
                            ..YearCounts::default()
                        },
                    );
                    &mut years[i]
                }
            };
            if kind == "Born" {
                entry.births += 1;
            } else {
                entry.deaths += 1;
            }
        }
        Ok(years)
    }

    /// Population snapshots, one per game year.
    pub fn yearly_snapshots(&self) -> Result<Vec<YearSnapshotRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT year, time, population, founders, couples, avg_age FROM yearly ORDER BY year",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(YearSnapshotRow {
                year: r.get(0)?,
                time: GameTime(r.get::<_, i64>(1)?.max(0) as u64),
                population: r.get(2)?,
                founders: r.get(3)?,
                couples: r.get(4)?,
                avg_age: r.get::<_, f64>(5)? as f32,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Stored events of a kind (by [`crate::sync::kind_name`]).
    pub fn count_kind(&self, kind: &str) -> Result<u64> {
        let n: i64 = self
            .conn
            .prepare_cached("SELECT COUNT(*) FROM events WHERE kind = ?1")?
            .query_row([kind], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Size of the history.
    pub fn counts(&self) -> Result<HistoryCounts> {
        Ok(self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM events),
                    (SELECT COUNT(*) FROM people),
                    (SELECT COUNT(*) FROM people WHERE died IS NULL),
                    (SELECT COUNT(*) FROM couples),
                    (SELECT TOTAL(to_seq - from_seq) FROM gaps),
                    (SELECT COUNT(*) FROM gaps)",
            [],
            |r| {
                let people = r.get::<_, i64>(1)? as u64;
                let alive = r.get::<_, i64>(2)? as u64;
                Ok(HistoryCounts {
                    events: r.get::<_, i64>(0)? as u64,
                    people,
                    alive,
                    dead: people - alive,
                    couples: r.get::<_, i64>(3)? as u64,
                    lost_events: r.get::<_, f64>(4)? as u64,
                    gaps: r.get::<_, i64>(5)? as u64,
                })
            },
        )?)
    }
}

/// `PERSON_COLS` with a table prefix.
fn prefixed(table: &str) -> String {
    PERSON_COLS
        .split(", ")
        .map(|c| format!("{table}.{c}"))
        .collect::<Vec<_>>()
        .join(", ")
}
