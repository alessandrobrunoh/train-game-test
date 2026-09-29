//! Lavori: identità e orari.
//!
//! A [`Job`] is, like [`crate::ItemKind`], a stable id in the world's
//! [`crate::Catalog`] (the 4 builtin jobs first, then the ones the Custode
//! adds) plus a handle to what never changes about it, its [`JobInfo`]:
//! names, workplace, station, shift and whether a tool helps. What the job
//! does ([`crate::Work`]) and how many people it wants are in the catalog
//! ([`crate::JobDef`]). Two jobs are equal when their ids are.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::carriage::{CarriageKind, StationKind};
use crate::defs::jobs::{BUILTIN_JOB_CODES, BUILTIN_JOB_INFOS};
use crate::time::GameTime;

/// What never changes about a job (see the module docs).
#[derive(Debug, PartialEq, Eq)]
pub struct JobInfo {
    /// Stable key: "contadino", or the normalized name of a new job.
    pub key: &'static str,
    /// Lowercase: "contadino".
    pub name: &'static str,
    /// Lowercase plural: "contadini".
    pub plural: &'static str,
    /// Kind of carriage where the job is done.
    pub workplace: CarriageKind,
    /// Station the worker occupies there.
    pub station: StationKind,
    /// Work shift as `[start, end)` hours, interrupted by the lunch break.
    pub shift: (u32, u32),
    /// Whether an owned Attrezzo boosts the output (and wears with work).
    pub uses_tool: bool,
}

/// A [`JobInfo`] with owned strings: how a new job's info is built and
/// serialized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobInfoData {
    pub key: String,
    pub name: String,
    pub plural: String,
    pub workplace: CarriageKind,
    pub station: StationKind,
    pub shift: (u32, u32),
    pub uses_tool: bool,
}

impl JobInfoData {
    fn of(info: &JobInfo) -> JobInfoData {
        JobInfoData {
            key: info.key.to_string(),
            name: info.name.to_string(),
            plural: info.plural.to_string(),
            workplace: info.workplace,
            station: info.station,
            shift: info.shift,
            uses_tool: info.uses_tool,
        }
    }

    fn matches(&self, info: &JobInfo) -> bool {
        self.key == info.key
            && self.name == info.name
            && self.plural == info.plural
            && self.workplace == info.workplace
            && self.station == info.station
            && self.shift == info.shift
            && self.uses_tool == info.uses_tool
    }
}

/// Infos of the jobs created during this process, shared by content.
static INTERNED: Mutex<Vec<&'static JobInfo>> = Mutex::new(Vec::new());

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn intern(data: &JobInfoData) -> &'static JobInfo {
    let mut interned = INTERNED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = interned.iter().find(|i| data.matches(i)) {
        return found;
    }
    let info: &'static JobInfo = Box::leak(Box::new(JobInfo {
        key: leak(&data.key),
        name: leak(&data.name),
        plural: leak(&data.plural),
        workplace: data.workplace,
        station: data.station,
        shift: data.shift,
        uses_tool: data.uses_tool,
    }));
    interned.push(info);
    info
}

/// A job: its id in the catalog and its [`JobInfo`].
#[derive(Clone, Copy)]
pub struct Job {
    id: u16,
    info: &'static JobInfo,
}

#[allow(non_upper_case_globals)]
impl Job {
    /// Works the grow beds of a Serra: grows Verdura.
    pub const Contadino: Job = Job::builtin(0);
    /// Works the kitchen of a Mensa: cooks Verdura from the Serre into Razioni.
    pub const Cuoco: Job = Job::builtin(1);
    /// Works a bench in an Officina: turns Rottame into Attrezzi and Vestiti.
    pub const Operaio: Job = Job::builtin(2);
    /// Works the counter of a Mercato: brings Attrezzi and Vestiti from the
    /// Officine to the Mercato.
    pub const Mercante: Job = Job::builtin(3);

    /// How many builtin jobs there are (ids `0..BUILTIN_COUNT`).
    pub const BUILTIN_COUNT: usize = 4;
    /// The builtin jobs, in id order. A world may have more: see
    /// [`crate::Catalog::job_kinds`].
    pub const BUILTIN: [Job; Self::BUILTIN_COUNT] =
        [Job::Contadino, Job::Cuoco, Job::Operaio, Job::Mercante];

    const fn builtin(id: u16) -> Job {
        Job {
            id,
            info: &BUILTIN_JOB_INFOS[id as usize],
        }
    }

    /// A new job with id `id` (at least [`Job::BUILTIN_COUNT`]).
    pub(crate) fn new(id: u16, info: &JobInfoData) -> Job {
        debug_assert!(usize::from(id) >= Self::BUILTIN_COUNT);
        Job {
            id,
            info: intern(info),
        }
    }

    /// Position in the catalog (and in per-job vectors).
    pub fn index(self) -> usize {
        usize::from(self.id)
    }

    pub fn id(self) -> u16 {
        self.id
    }

    pub fn is_builtin(self) -> bool {
        self.index() < Self::BUILTIN_COUNT
    }

    pub fn info(self) -> &'static JobInfo {
        self.info
    }

    pub fn key(self) -> &'static str {
        self.info.key
    }

    /// Code of a builtin job as in the saves and in `Debug`: "Contadino";
    /// the key for a new one.
    pub fn code(self) -> &'static str {
        BUILTIN_JOB_CODES
            .get(self.index())
            .copied()
            .unwrap_or(self.info.key)
    }

    /// The builtin job with this code ("Contadino"), if any.
    pub fn from_code(code: &str) -> Option<Job> {
        BUILTIN_JOB_CODES
            .iter()
            .position(|&c| c == code)
            .map(|i| Job::BUILTIN[i])
    }

    pub fn name(self) -> &'static str {
        self.info.name
    }

    pub fn plural(self) -> &'static str {
        self.info.plural
    }

    pub fn workplace_kind(self) -> CarriageKind {
        self.info.workplace
    }

    pub fn station_kind(self) -> StationKind {
        self.info.station
    }

    /// Whether an Attrezzo boosts (and wears with) this job's work.
    pub fn uses_tool(self) -> bool {
        self.info.uses_tool
    }

    /// Work shift as `[start, end)` hours, interrupted by [`Job::LUNCH_BREAK`].
    pub fn shift(self) -> (u32, u32) {
        self.info.shift
    }

    /// Default lunch break `[start, end)` hours: no work, everyone gets a
    /// chance to eat. Each worker actually breaks at its meal shift's lunch
    /// ([`crate::SimParams::lunch_break`], see [`Job::works_at`]).
    pub const LUNCH_BREAK: (u32, u32) = (12, 13);

    /// Whether `time` falls in working hours (shift minus the default lunch break).
    pub fn in_shift(self, time: GameTime) -> bool {
        self.works_at(time, Self::default_lunch())
    }

    /// Minutes until the current stretch of work ends (default lunch break or
    /// end of shift); 0 outside working hours.
    pub fn shift_minutes_left(self, time: GameTime) -> u64 {
        self.minutes_left_at(time, Self::default_lunch())
    }

    fn default_lunch() -> (u32, u32) {
        (Self::LUNCH_BREAK.0 * 60, Self::LUNCH_BREAK.1 * 60)
    }

    /// Whether `time` falls in working hours with a lunch break of
    /// `[start, end)` minutes of the day.
    pub fn works_at(self, time: GameTime, lunch: (u32, u32)) -> bool {
        let (start, end) = self.shift();
        let now = time.minute_of_day();
        (start * 60..end * 60).contains(&now) && !(lunch.0..lunch.1).contains(&now)
    }

    /// Minutes until the current stretch of work ends (the lunch break
    /// `[start, end)` in minutes of the day, or the end of the shift); 0
    /// outside working hours.
    pub fn minutes_left_at(self, time: GameTime, lunch: (u32, u32)) -> u64 {
        if !self.works_at(time, lunch) {
            return 0;
        }
        let now = time.minute_of_day();
        let end = if now < lunch.0 {
            lunch.0.min(self.shift().1 * 60)
        } else {
            self.shift().1 * 60
        };
        u64::from(end - now)
    }
}

impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Job {}

impl Hash for Job {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Job {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_builtin() {
            f.write_str(self.code())
        } else {
            write!(f, "Nuovo({}#{})", self.info.key, self.id)
        }
    }
}

impl fmt::Display for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Serialize, Deserialize)]
struct Wire {
    id: u16,
    #[serde(default)]
    info: Option<JobInfoData>,
}

impl Wire {
    fn of(job: Job) -> Wire {
        Wire {
            id: job.id,
            info: (!job.is_builtin()).then(|| JobInfoData::of(job.info)),
        }
    }

    fn job<E: serde::de::Error>(self) -> Result<Job, E> {
        if usize::from(self.id) < Job::BUILTIN_COUNT {
            return Ok(Job::builtin(self.id));
        }
        match self.info {
            Some(info) => Ok(Job::new(self.id, &info)),
            None => Err(E::custom(format!("lavoro {} senza descrizione", self.id))),
        }
    }
}

impl Serialize for Job {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() && self.is_builtin() {
            s.serialize_str(self.code())
        } else {
            Wire::of(*self).serialize(s)
        }
    }
}

impl<'de> Deserialize<'de> for Job {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        if !d.is_human_readable() {
            return Wire::deserialize(d)?.job();
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Human {
            Code(String),
            Wire(Wire),
        }
        match Human::deserialize(d)? {
            Human::Code(code) => Job::from_code(&code)
                .ok_or_else(|| D::Error::custom(format!("lavoro sconosciuto «{code}»"))),
            Human::Wire(w) => w.job(),
        }
    }
}
