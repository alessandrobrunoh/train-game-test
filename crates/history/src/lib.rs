//! Storico persistente della simulazione in SQLite (`<run dir>/history.sqlite`).
//!
//! The sim keeps only the most recent events in memory ([`sim::World::events`],
//! capped at `max_events`). This crate appends every event to a SQLite
//! database as it happens, plus derived tables (people, couples, yearly
//! snapshots) that outlive the world: dead NPCs stay queryable forever, family
//! trees span all generations, the player's deeds are never forgotten.
//!
//! # Usage
//!
//! ```no_run
//! # fn main() -> Result<(), history::Error> {
//! # let mut world = sim::World::generate(1, 10, 100);
//! let mut history = history::History::open(std::path::Path::new("runs/abc"))?;
//! history.sync(&world)?; // after some ticks: appends the new events
//! let bio = history.biography(sim::NpcId(3))?;
//! # Ok(()) }
//! ```
//!
//! [`History::sync`] is incremental and idempotent: it appends the events
//! with a global index (`seq`, the position in [`sim::World::events_total`]
//! order) `>= synced`, in one transaction. Call it more often than the event
//! log trims (see [`SyncBatch::capture`]): events dropped from memory before
//! they were synced can't be recovered and are recorded as a gap.
//! For writing on another thread, split it: [`SyncBatch::capture`] (cheap,
//! needs the world) and [`History::apply`] (the SQL, only needs the batch).
//!
//! Loading an older save: call [`History::reconcile`] (or
//! [`History::rollback_to`] with its `events_total()`) so the "future" that
//! never happened is deleted.
//!
//! # Schema (version 1)
//!
//! ```sql
//! schema_version(version)                   -- one row: the applied migrations
//! meta(key PRIMARY KEY, value INTEGER)      -- 'synced': next seq to append;
//!                                           -- 'seeded_at': first seq whose effects are
//!                                           -- recorded (people were seeded from the world then)
//! events(seq INTEGER PRIMARY KEY,           -- global event index (0-based)
//!        time INTEGER,                      -- game minutes since day 1 00:00
//!        kind TEXT,                         -- EventKind variant name, e.g. 'Born'
//!        npc INTEGER NULL,                  -- main NPC (child, dead, buyer, mother...)
//!        other INTEGER NULL,                -- second NPC (partner, mother of a newborn...)
//!        carriage INTEGER NULL,             -- CarriageId (0-based)
//!        item TEXT NULL,                    -- ItemKind variant name, e.g. 'Razione'
//!        amount INTEGER NULL,               -- units (items), age (deaths)
//!        price INTEGER NULL,                -- tokens paid
//!        text TEXT,                         -- Italian message (Display, without the time)
//!        data TEXT)                         -- JSON of the EventKind (serde)
//!   INDEX (kind, time)
//! event_npcs(npc, seq, PRIMARY KEY (npc, seq)) WITHOUT ROWID
//!                                           -- every NPC involved in an event (a newborn's
//!                                           -- father too): the index behind biographies
//! people(id INTEGER PRIMARY KEY, name, sex 'F'|'M',
//!        born INTEGER NULL,                 -- game minutes, negative for founders; NULL if unknown
//!        died INTEGER NULL, death_cause TEXT NULL ('OldAge'|'Starvation'), death_age INTEGER NULL,
//!        mother INTEGER NULL, father INTEGER NULL, founder INTEGER (0|1),
//!        born_seq INTEGER NULL, died_seq INTEGER NULL)  -- events that set born/died (for rollback)
//!   INDEX (mother), INDEX (father)
//! couples(a, b, formed INTEGER NULL, formed_seq NULL, ended INTEGER NULL, ended_seq NULL,
//!         PRIMARY KEY (a, b)) WITHOUT ROWID -- a < b; formed NULL: already a couple at seeding
//!   INDEX (b)
//! yearly(year INTEGER PRIMARY KEY, time, seq, population, founders, couples, avg_age)
//!                                           -- first sync of each game year
//! gaps(from_seq INTEGER PRIMARY KEY, to_seq, time)  -- events lost before a sync: [from, to)
//! ```
//!
//! People are seeded from the living NPCs at the first sync (founders and
//! anyone else alive), then kept up to date from `Born` / `NpcDied` events.

mod query;
mod sync;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

pub use query::{
    CoupleRow, EventRow, FamilyNode, FamilyTree, HistoryCounts, PersonRow, PlayerTotal, YearCounts,
    YearSnapshotRow,
};
pub use sync::{ReconcileReport, SyncBatch, SyncReport, YearSnapshot};

/// Name of the database file inside the run directory.
pub const DB_FILE: &str = "history.sqlite";

/// Migrations by version: `MIGRATIONS[i]` brings the schema to version `i + 1`.
const MIGRATIONS: &[&str] = &[
    // v1
    "
    CREATE TABLE meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID;
    CREATE TABLE events (
        seq INTEGER PRIMARY KEY,
        time INTEGER NOT NULL,
        kind TEXT NOT NULL,
        npc INTEGER,
        other INTEGER,
        carriage INTEGER,
        item TEXT,
        amount INTEGER,
        price INTEGER,
        text TEXT NOT NULL,
        data TEXT NOT NULL
    );
    CREATE INDEX events_kind_time ON events(kind, time);
    CREATE TABLE event_npcs (
        npc INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        PRIMARY KEY (npc, seq)
    ) WITHOUT ROWID;
    CREATE TABLE people (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        sex TEXT NOT NULL CHECK (sex IN ('F', 'M')),
        born INTEGER,
        died INTEGER,
        death_cause TEXT,
        death_age INTEGER,
        mother INTEGER,
        father INTEGER,
        founder INTEGER NOT NULL DEFAULT 0,
        born_seq INTEGER,
        died_seq INTEGER
    );
    CREATE INDEX people_mother ON people(mother) WHERE mother IS NOT NULL;
    CREATE INDEX people_father ON people(father) WHERE father IS NOT NULL;
    CREATE TABLE couples (
        a INTEGER NOT NULL,
        b INTEGER NOT NULL,
        formed INTEGER,
        formed_seq INTEGER,
        ended INTEGER,
        ended_seq INTEGER,
        PRIMARY KEY (a, b)
    ) WITHOUT ROWID;
    CREATE INDEX couples_b ON couples(b);
    CREATE TABLE yearly (
        year INTEGER PRIMARY KEY,
        time INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        population INTEGER NOT NULL,
        founders INTEGER NOT NULL,
        couples INTEGER NOT NULL,
        avg_age REAL NOT NULL
    );
    CREATE TABLE gaps (
        from_seq INTEGER PRIMARY KEY,
        to_seq INTEGER NOT NULL,
        time INTEGER NOT NULL
    );
    ",
];

/// Schema version this crate writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

#[derive(Debug)]
pub enum Error {
    Sql(rusqlite::Error),
    Io(std::io::Error),
    /// The database was written by a newer version of the game.
    NewerSchema(u32),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Sql(e) => write!(f, "SQLite: {e}"),
            Error::Io(e) => write!(f, "I/O: {e}"),
            Error::NewerSchema(v) => write!(
                f,
                "schema versione {v}, più recente di quella supportata ({SCHEMA_VERSION})"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Sql(e) => Some(e),
            Error::Io(e) => Some(e),
            Error::NewerSchema(_) => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sql(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// A connection to a run's history database.
pub struct History {
    conn: Connection,
    path: Option<PathBuf>,
    /// Next event seq to append (cached `meta.synced`).
    synced: u64,
    /// `meta.seeded_at`: `None` until the first sync seeded the people.
    seeded_at: Option<u64>,
}

impl fmt::Debug for History {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("History")
            .field("path", &self.path)
            .field("synced", &self.synced)
            .field("seeded_at", &self.seeded_at)
            .finish()
    }
}

impl History {
    /// Opens (or creates, with its directory) `<dir>/history.sqlite` for
    /// reading and writing, migrating the schema if needed.
    pub fn open(dir: &Path) -> Result<History> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(DB_FILE);
        let conn = Connection::open(&path)?;
        // WAL: readers (the UI) never block the writer; NORMAL: no fsync per
        // commit, only at checkpoints (a crash loses at most the last commits).
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
        Self::init(conn, Some(path))
    }

    /// Opens an existing database read-only (queries only), e.g. for a UI
    /// thread while another connection writes. Fails if it doesn't exist.
    pub fn open_read_only(dir: &Path) -> Result<History> {
        let path = dir.join(DB_FILE);
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(Duration::from_millis(200))?;
        let version = schema_version(&conn)?;
        if version != SCHEMA_VERSION {
            return Err(Error::NewerSchema(version));
        }
        let mut history = History {
            conn,
            path: Some(path),
            synced: 0,
            seeded_at: None,
        };
        history.load_meta()?;
        Ok(history)
    }

    /// A fresh database in memory (tests, tools).
    pub fn open_in_memory() -> Result<History> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, path: Option<PathBuf>) -> Result<History> {
        migrate(&conn)?;
        let mut history = History {
            conn,
            path,
            synced: 0,
            seeded_at: None,
        };
        history.load_meta()?;
        Ok(history)
    }

    /// Re-reads `synced` / `seeded_at` (a read-only connection sees what the
    /// writer committed).
    pub fn load_meta(&mut self) -> Result<()> {
        self.synced = get_meta(&self.conn, "synced")?.unwrap_or(0);
        self.seeded_at = get_meta(&self.conn, "seeded_at")?;
        Ok(())
    }

    /// Path of the database file (`None` in memory).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Next event seq to append: every event with a lower seq was synced
    /// (or lost in a gap).
    pub fn synced(&self) -> u64 {
        self.synced
    }

    /// Whether people were already seeded from a world (by the first sync).
    pub fn is_seeded(&self) -> bool {
        self.seeded_at.is_some()
    }

    /// The underlying connection, for ad-hoc queries.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

fn schema_version(conn: &Connection) -> Result<u32> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_version')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let version: Option<u32> = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .optional()?
        .flatten();
    Ok(version.unwrap_or(0))
}

/// Applies the missing migrations, each in its own transaction.
fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)",
    )?;
    let current = schema_version(conn)?;
    if current > SCHEMA_VERSION {
        return Err(Error::NewerSchema(current));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            params![i as u32 + 1, now],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn get_meta(conn: &Connection, key: &str) -> Result<Option<u64>> {
    let value: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?;
    Ok(value.map(|v| v.max(0) as u64))
}

fn set_meta(conn: &Connection, key: &str, value: u64) -> Result<()> {
    conn.prepare_cached("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)")?
        .execute(params![key, value as i64])?;
    Ok(())
}

#[cfg(test)]
mod tests;
