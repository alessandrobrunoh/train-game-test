//! Formato dei salvataggi e cartelle delle partite (dati puri, niente Bevy).
//!
//! Ogni partita ("run") ha una cartella `<dati>/runs/<run_id>/`, con i
//! salvataggi in `saves/<slot>.sav` (e lo storico SQLite di `history`). La
//! cartella dati è quella della piattaforma (su macOS
//! `~/Library/Application Support/TrainGame`), o `$TRAINGAME_DATA_DIR`.
//!
//! Un file `.sav` è fatto così:
//!
//! | byte       | contenuto                                                   |
//! |------------|-------------------------------------------------------------|
//! | 0..8       | `MAGIC` (`TRAINSAV`)                                        |
//! | 8..12      | versione del formato (u32 little endian)                    |
//! | 12..16     | lunghezza dell'intestazione (u32 LE)                        |
//! | ...        | intestazione [`SaveHeader`] (postcard), per l'elenco        |
//! | 4 byte     | lunghezza del corpo non compresso (u32 LE)                  |
//! | resto      | corpo [`SaveBody`] (postcard) compresso con LZ4 (blocco)    |
//!
//! L'intestazione si legge senza decomprimere né decodificare il mondo.
//! postcard non descrive i campi: ogni modifica a `SaveHeader`, `SaveBody` o
//! ai tipi serializzati della sim (`World`, `UtilityBrain`, ...) richiede di
//! alzare [`SAVE_VERSION`]. I file di un'altra versione vengono rifiutati.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sim::{GameTime, Stock, UtilityBrain, World};

/// Versione del formato dei salvataggi. 6: catalogo degli oggetti e crafting
/// (Fase 3: `Stock` con 13 oggetti) insieme a prezzi per distanza e
/// specialità delle carrozze (Fase 6a/6b). 7: piani delle carrozze
/// (`Npc::floor`, `Station::floor`, `SimParams::stairs_minutes`).
pub const SAVE_VERSION: u32 = 7;
const MAGIC: [u8; 8] = *b"TRAINSAV";
/// Byte fissi prima dell'intestazione: magic, versione, lunghezza.
const PREFIX_LEN: usize = MAGIC.len() + 4 + 4;
/// Limiti di sicurezza contro file danneggiati (niente allocazioni enormi).
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 1 << 30;

/// Variabile d'ambiente che sostituisce la cartella dati della piattaforma.
pub const DATA_DIR_ENV: &str = "TRAINGAME_DATA_DIR";
/// Estensione dei salvataggi.
pub const SAVE_EXT: &str = "sav";
/// Lunghezza massima del nome di uno slot.
pub const MAX_SLOT_CHARS: usize = 32;
/// Slot dei salvataggi automatici, a rotazione: `auto-1`, `auto-2`, ...
pub const AUTO_SLOTS: usize = 3;

// --- Errori --------------------------------------------------------------------

#[derive(Debug)]
pub enum SaveError {
    Io(io::Error),
    /// Il file non comincia con `MAGIC`.
    NotASave,
    /// Formato di un'altra versione del gioco.
    Version(u32),
    Corrupt(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "errore del disco: {e}"),
            SaveError::NotASave => write!(f, "il file non è un salvataggio di TrainGame"),
            SaveError::Version(v) => write!(
                f,
                "salvataggio in formato {v}, questa versione del gioco legge solo il formato {SAVE_VERSION}"
            ),
            SaveError::Corrupt(what) => write!(f, "salvataggio danneggiato ({what})"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        SaveError::Io(e)
    }
}

fn corrupt(what: impl fmt::Display) -> SaveError {
    SaveError::Corrupt(what.to_string())
}

// --- Contenuto ---------------------------------------------------------------------

/// Riassunto di un salvataggio, leggibile senza decodificare il mondo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveHeader {
    pub run_id: String,
    pub slot: String,
    /// Istante reale del salvataggio (millisecondi Unix).
    pub created_ms: u64,
    /// Ora di gioco.
    pub clock: GameTime,
    pub days_per_year: u32,
    pub population: u32,
    pub carriages: u32,
}

impl SaveHeader {
    pub fn of(run_id: &str, slot: &str, created_ms: u64, world: &World) -> Self {
        Self {
            run_id: run_id.to_string(),
            slot: slot.to_string(),
            created_ms,
            clock: world.clock,
            days_per_year: world.params.days_per_year,
            population: world.npcs.len() as u32,
            carriages: world.carriages.len() as u32,
        }
    }

    /// "Anno 3 Giorno 5 · 14:20" (giorno dell'anno, da 1).
    pub fn game_time_label(&self) -> String {
        let per_year = u64::from(self.days_per_year.max(1));
        let day = self.clock.day() - 1;
        format!(
            "Anno {} Giorno {} · {:02}:{:02}",
            day / per_year + 1,
            day % per_year + 1,
            self.clock.hour(),
            self.clock.minutes() % 60
        )
    }
}

/// Corpo del salvataggio in scrittura: prende in prestito il mondo, così non
/// serve clonarlo. Si codifica esattamente come [`SaveBody`] (stessi campi,
/// stesso ordine: un riferimento si serializza come il valore).
#[derive(Serialize)]
pub struct SaveBodyRef<'a> {
    pub world: &'a World,
    pub brain: &'a UtilityBrain,
    /// Posizione del giocatore (centro del corpo).
    pub player: [f32; 2],
    /// Inventario del giocatore.
    pub tokens: u32,
    pub items: Stock,
    /// Ricette imparate oltre a quelle di base (chiavi).
    pub learnt_recipes: &'a [String],
    /// Velocità del tempo (minuti di gioco al secondo) e pausa.
    pub minutes_per_second: f32,
    pub paused: bool,
}

/// Corpo del salvataggio letto dal disco (vedi [`SaveBodyRef`]).
#[derive(Debug, Deserialize)]
pub struct SaveBody {
    pub world: World,
    pub brain: UtilityBrain,
    pub player: [f32; 2],
    pub tokens: u32,
    pub items: Stock,
    pub learnt_recipes: Vec<String>,
    pub minutes_per_second: f32,
    /// Se era in pausa: salvato per completezza, ma una partita caricata
    /// riparte sempre in pausa.
    #[allow(dead_code)]
    pub paused: bool,
}

/// Un salvataggio completo (la versione è quella del file, [`SAVE_VERSION`]).
#[derive(Debug)]
pub struct SaveFile {
    pub header: SaveHeader,
    pub body: SaveBody,
}

// --- Codifica ---------------------------------------------------------------------

/// Salvataggio serializzato ma non ancora compresso: la serializzazione deve
/// avvenire dove c'è il mondo (thread principale), compressione e scrittura
/// possono andare altrove ([`EncodedSave::finish`]).
pub struct EncodedSave {
    header: Vec<u8>,
    body: Vec<u8>,
}

pub fn encode(header: &SaveHeader, body: &SaveBodyRef) -> Result<EncodedSave, SaveError> {
    let header = postcard::to_allocvec(header).map_err(corrupt)?;
    let body = postcard::to_allocvec(body).map_err(corrupt)?;
    if header.len() > MAX_HEADER_BYTES || body.len() > MAX_BODY_BYTES {
        return Err(corrupt("troppo grande"));
    }
    Ok(EncodedSave { header, body })
}

impl EncodedSave {
    /// Byte del corpo non compresso.
    pub fn raw_len(&self) -> usize {
        self.body.len()
    }

    /// Comprime il corpo e compone il file.
    pub fn finish(self) -> Vec<u8> {
        let compressed = lz4_flex::block::compress(&self.body);
        let mut out = Vec::with_capacity(PREFIX_LEN + self.header.len() + 4 + compressed.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&SAVE_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.header.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.header);
        out.extend_from_slice(&(self.body.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
        out
    }
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

/// Controlla magic e versione; restituisce la lunghezza dell'intestazione.
fn check_prefix(prefix: &[u8]) -> Result<usize, SaveError> {
    if prefix.len() < PREFIX_LEN || prefix[..MAGIC.len()] != MAGIC {
        return Err(SaveError::NotASave);
    }
    let version = read_u32(prefix, MAGIC.len()).ok_or(SaveError::NotASave)?;
    if version != SAVE_VERSION {
        return Err(SaveError::Version(version));
    }
    let len = read_u32(prefix, MAGIC.len() + 4).ok_or(SaveError::NotASave)? as usize;
    if len > MAX_HEADER_BYTES {
        return Err(corrupt("intestazione troppo lunga"));
    }
    Ok(len)
}

/// Decodifica un salvataggio intero.
pub fn decode(bytes: &[u8]) -> Result<SaveFile, SaveError> {
    let header_len = check_prefix(bytes)?;
    let header_bytes = bytes
        .get(PREFIX_LEN..PREFIX_LEN + header_len)
        .ok_or_else(|| corrupt("file troncato"))?;
    let header: SaveHeader = postcard::from_bytes(header_bytes).map_err(corrupt)?;
    let rest = &bytes[PREFIX_LEN + header_len..];
    let raw_len = read_u32(rest, 0).ok_or_else(|| corrupt("file troncato"))? as usize;
    if raw_len > MAX_BODY_BYTES {
        return Err(corrupt("corpo troppo grande"));
    }
    let raw = lz4_flex::block::decompress(&rest[4..], raw_len).map_err(corrupt)?;
    if raw.len() != raw_len {
        return Err(corrupt("lunghezza del corpo errata"));
    }
    let body: SaveBody = postcard::from_bytes(&raw).map_err(corrupt)?;
    Ok(SaveFile { header, body })
}

/// Legge solo l'intestazione di un salvataggio (per l'elenco).
pub fn read_header(path: &Path) -> Result<SaveHeader, SaveError> {
    let mut file = File::open(path)?;
    let mut prefix = [0u8; PREFIX_LEN];
    file.read_exact(&mut prefix)
        .map_err(|_| SaveError::NotASave)?;
    let len = check_prefix(&prefix)?;
    let mut header = vec![0u8; len];
    file.read_exact(&mut header)
        .map_err(|_| corrupt("file troncato"))?;
    postcard::from_bytes(&header).map_err(corrupt)
}

pub fn read_save(path: &Path) -> Result<SaveFile, SaveError> {
    decode(&fs::read(path)?)
}

/// Scrive in un file temporaneo accanto a `path` e poi lo rinomina: un
/// salvataggio interrotto non rovina quello precedente.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("{SAVE_EXT}.tmp{}-{n}", std::process::id()));
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

// --- Cartelle ------------------------------------------------------------------------

/// Millisecondi Unix di adesso.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Cartella dati del gioco: `$TRAINGAME_DATA_DIR` se impostata, altrimenti
/// quella della piattaforma.
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("", "", "TrainGame")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("traingame-data"))
}

pub fn runs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("runs")
}

pub fn saves_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("saves")
}

pub fn slot_path(run_dir: &Path, slot: &str) -> PathBuf {
    saves_dir(run_dir).join(format!("{slot}.{SAVE_EXT}"))
}

pub fn auto_slot(n: usize) -> String {
    format!("auto-{n}")
}

/// Id di una run: `seed42-1790000000` (seme e secondi Unix della creazione).
pub fn run_id(seed: u64, unix_secs: u64) -> String {
    format!("seed{seed}-{unix_secs}")
}

/// Seme e secondi di creazione di una run, dal suo id.
pub fn parse_run_id(run_id: &str) -> Option<(u64, u64)> {
    let mut parts = run_id.strip_prefix("seed")?.split('-');
    let seed = parts.next()?.parse().ok()?;
    let secs = parts.next()?.parse().ok()?;
    Some((seed, secs))
}

/// Crea la cartella di una nuova run (con un suffisso se l'id è già usato).
pub fn create_run(data_dir: &Path, seed: u64, unix_secs: u64) -> io::Result<(String, PathBuf)> {
    let runs = runs_dir(data_dir);
    fs::create_dir_all(&runs)?;
    let base = run_id(seed, unix_secs);
    for n in 1..1000 {
        let id = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let dir = runs.join(&id);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("troppe partite con lo stesso id"))
}

/// Nome di slot sicuro per il filesystem: lettere, cifre, `-` e `_`; gli
/// spazi diventano `-`. `None` se non resta niente.
pub fn sanitize_slot(name: &str) -> Option<String> {
    let slot: String = name
        .trim()
        .chars()
        .filter_map(|c| match c {
            c if c.is_alphanumeric() => Some(c),
            '-' | '_' => Some(c),
            c if c.is_whitespace() => Some('-'),
            _ => None,
        })
        .take(MAX_SLOT_CHARS)
        .collect();
    (!slot.is_empty()).then_some(slot)
}

// --- Elenco ---------------------------------------------------------------------------

/// Un file di salvataggio con la sua intestazione (o l'errore di lettura).
#[derive(Debug)]
pub struct SaveEntry {
    pub path: PathBuf,
    pub slot: String,
    pub header: Result<SaveHeader, String>,
}

impl SaveEntry {
    pub fn created_ms(&self) -> u64 {
        self.header.as_ref().map_or(0, |h| h.created_ms)
    }
}

/// Una run con i suoi salvataggi, dal più recente.
#[derive(Debug)]
pub struct RunEntry {
    pub run_id: String,
    pub dir: PathBuf,
    pub saves: Vec<SaveEntry>,
}

impl RunEntry {
    /// Istante più recente noto della run: ultimo salvataggio o creazione.
    pub fn latest_ms(&self) -> u64 {
        let created = parse_run_id(&self.run_id).map_or(0, |(_, secs)| secs * 1000);
        self.saves
            .iter()
            .map(SaveEntry::created_ms)
            .fold(created, u64::max)
    }
}

/// Salvataggi di una run, dal più recente (quelli illeggibili in fondo).
pub fn list_saves(run_dir: &Path) -> Vec<SaveEntry> {
    let Ok(entries) = fs::read_dir(saves_dir(run_dir)) else {
        return Vec::new();
    };
    let mut saves: Vec<SaveEntry> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == SAVE_EXT))
        .map(|path| SaveEntry {
            slot: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            header: read_header(&path).map_err(|e| e.to_string()),
            path,
        })
        .collect();
    saves.sort_by(|a, b| {
        b.created_ms()
            .cmp(&a.created_ms())
            .then_with(|| a.slot.cmp(&b.slot))
    });
    saves
}

/// Tutte le run, dalla più recente.
pub fn list_runs(data_dir: &Path) -> Vec<RunEntry> {
    let Ok(entries) = fs::read_dir(runs_dir(data_dir)) else {
        return Vec::new();
    };
    let mut runs: Vec<RunEntry> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .map(|dir| RunEntry {
            run_id: dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            saves: list_saves(&dir),
            dir,
        })
        .collect();
    runs.sort_by(|a, b| {
        b.latest_ms()
            .cmp(&a.latest_ms())
            .then_with(|| b.run_id.cmp(&a.run_id))
    });
    runs
}

/// Il salvataggio leggibile più recente di una run.
pub fn latest_save(run_dir: &Path) -> Option<SaveEntry> {
    list_saves(run_dir).into_iter().find(|s| s.header.is_ok())
}

/// Prossimo slot automatico: il primo che manca, altrimenti il più vecchio.
pub fn next_auto_slot(run_dir: &Path) -> String {
    (1..=AUTO_SLOTS)
        .map(|n| {
            let slot = auto_slot(n);
            let created = read_header(&slot_path(run_dir, &slot))
                .ok()
                .map(|h| h.created_ms);
            (created, slot)
        })
        .min_by_key(|(created, _)| created.unwrap_or(0))
        .map(|(_, slot)| slot)
        .unwrap_or_else(|| auto_slot(1))
}

/// Elimina la cartella di una run (solo se sta davvero sotto `runs/`).
pub fn delete_run(data_dir: &Path, run_dir: &Path) -> io::Result<()> {
    let runs = runs_dir(data_dir).canonicalize()?;
    let dir = run_dir.canonicalize()?;
    if dir.parent() != Some(runs.as_path()) {
        return Err(io::Error::other("la cartella non è una partita"));
    }
    fs::remove_dir_all(dir)
}

/// Run a cui appartiene un file di salvataggio (`<run>/saves/<slot>.sav`).
pub fn run_of_save(path: &Path) -> Option<(String, PathBuf)> {
    let dir = path.parent()?.parent()?;
    let id = dir.file_name()?.to_string_lossy().into_owned();
    Some((id, dir.to_path_buf()))
}

// --- Date -------------------------------------------------------------------------------

/// Da quanto tempo, in parole: "adesso", "5 min fa", "3 h fa", "ieri", "4 giorni fa".
pub fn format_age(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..60 => "adesso".to_string(),
        60..3600 => format!("{} min fa", secs / 60),
        3600..86_400 => format!("{} h fa", secs / 3600),
        86_400..172_800 => "ieri".to_string(),
        _ => format!("{} giorni fa", secs / 86_400),
    }
}

/// Data e ora UTC, es. "28/09/2026 14:03 UTC".
pub fn format_utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let minute_of_day = secs % 86_400 / 60;
    format!(
        "{d:02}/{m:02}/{y} {:02}:{:02} UTC",
        minute_of_day / 60,
        minute_of_day % 60
    )
}

/// Giorni dal 1970-01-01 -> (anno, mese, giorno) del calendario gregoriano
/// (algoritmo di H. Hinnant).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sim::{ItemKind, MINUTES_PER_DAY};

    /// Cartella temporanea eliminata a fine test.
    pub(crate) struct TempDir(pub(crate) PathBuf);

    impl TempDir {
        pub(crate) fn new(name: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("traingame-test-{}-{name}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn world_bytes(world: &World) -> Vec<u8> {
        postcard::to_allocvec(world).unwrap()
    }

    fn sample(world: &World, brain: &UtilityBrain, slot: &str) -> Vec<u8> {
        let mut items = Stock::default();
        items.set(ItemKind::Razione, 3.0);
        items.set(ItemKind::Coperta, 2.0);
        let learnt = ["lampada".to_string()];
        let header = SaveHeader::of("seed7-100", slot, 1234, world);
        let body = SaveBodyRef {
            world,
            brain,
            player: [12.5, 24.0],
            tokens: 42,
            items,
            learnt_recipes: &learnt,
            minutes_per_second: 600.0,
            paused: false,
        };
        encode(&header, &body).unwrap().finish()
    }

    #[test]
    fn roundtrip_is_exact() {
        let mut world = World::generate(7, 10, 120);
        let mut brain = UtilityBrain::new(7);
        world.run(&mut brain, 2 * MINUTES_PER_DAY + 77);
        let bytes = sample(&world, &brain, "prova");
        let file = decode(&bytes).unwrap();
        assert_eq!(file.header.slot, "prova");
        assert_eq!(file.header.run_id, "seed7-100");
        assert_eq!(file.header.population, world.npcs.len() as u32);
        assert_eq!(file.header.clock, world.clock);
        assert_eq!(world_bytes(&file.body.world), world_bytes(&world));
        assert_eq!(
            postcard::to_allocvec(&file.body.brain).unwrap(),
            postcard::to_allocvec(&brain).unwrap()
        );
        assert_eq!(file.body.player, [12.5, 24.0]);
        assert_eq!(file.body.tokens, 42);
        assert_eq!(file.body.items.count(ItemKind::Razione), 3);
        assert_eq!(file.body.items.count(ItemKind::Coperta), 2);
        assert_eq!(file.body.learnt_recipes, ["lampada"]);
        assert_eq!(file.body.minutes_per_second, 600.0);
        // Anche l'evento più vecchio e i contatori interni.
        assert_eq!(file.body.world.events_total(), world.events_total());
    }

    #[test]
    fn refuses_other_versions_and_garbage() {
        let world = World::generate(1, 3, 10);
        let mut bytes = sample(&world, &UtilityBrain::new(1), "x");
        bytes[8..12].copy_from_slice(&(SAVE_VERSION + 1).to_le_bytes());
        let err = decode(&bytes).unwrap_err();
        assert!(matches!(err, SaveError::Version(v) if v == SAVE_VERSION + 1));
        assert!(err.to_string().contains("formato"), "{err}");
        assert!(matches!(decode(b"ciao"), Err(SaveError::NotASave)));
        // Troncato: errore, non panico.
        let good = sample(&world, &UtilityBrain::new(1), "x");
        for len in [PREFIX_LEN, PREFIX_LEN + 5, good.len() / 2, good.len() - 1] {
            assert!(decode(&good[..len]).is_err(), "len {len}");
        }
    }

    #[test]
    fn header_is_read_without_the_body_and_writes_are_atomic() {
        let tmp = TempDir::new("header");
        let run = tmp.0.join("runs").join("seed7-100");
        let world = World::generate(1, 3, 10);
        let path = slot_path(&run, "rapido");
        write_atomic(&path, &sample(&world, &UtilityBrain::new(1), "rapido")).unwrap();
        let header = read_header(&path).unwrap();
        assert_eq!(header.slot, "rapido");
        assert_eq!(header.carriages, 3);
        // Solo il file finale, niente temporanei.
        let files: Vec<_> = fs::read_dir(saves_dir(&run)).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        // Sovrascrivere sostituisce.
        write_atomic(&path, &sample(&world, &UtilityBrain::new(1), "altro")).unwrap();
        assert_eq!(read_header(&path).unwrap().slot, "altro");
        assert_eq!(read_save(&path).unwrap().header.slot, "altro");
    }

    #[test]
    fn lists_runs_and_saves_newest_first() {
        let tmp = TempDir::new("list");
        let data = &tmp.0;
        let (old_id, old_run) = create_run(data, 5, 1000).unwrap();
        let (new_id, new_run) = create_run(data, 5, 1000).unwrap();
        assert_eq!(old_id, "seed5-1000");
        assert_eq!(new_id, "seed5-1000-2");
        assert_eq!(parse_run_id(&new_id), Some((5, 1000)));
        let world = World::generate(1, 3, 10);
        let brain = UtilityBrain::new(1);
        let save = |run: &Path, slot: &str, ms: u64| {
            let header = SaveHeader::of("x", slot, ms, &world);
            let body = SaveBodyRef {
                world: &world,
                brain: &brain,
                player: [0.0; 2],
                tokens: 0,
                items: Stock::default(),
                learnt_recipes: &[],
                minutes_per_second: 1.0,
                paused: true,
            };
            let bytes = encode(&header, &body).unwrap().finish();
            write_atomic(&slot_path(run, slot), &bytes).unwrap();
        };
        save(&old_run, "a", 5_000_000);
        save(&new_run, "b", 2_000_000);
        save(&new_run, "c", 3_000_000);
        fs::write(slot_path(&new_run, "rotto"), b"non un salvataggio").unwrap();
        fs::write(saves_dir(&new_run).join("note.txt"), b"ignorato").unwrap();

        let runs = list_runs(data);
        let ids: Vec<&str> = runs.iter().map(|r| r.run_id.as_str()).collect();
        assert_eq!(ids, ["seed5-1000", "seed5-1000-2"]);
        let slots: Vec<&str> = runs[1].saves.iter().map(|s| s.slot.as_str()).collect();
        assert_eq!(slots, ["c", "b", "rotto"]);
        assert!(runs[1].saves[2].header.is_err());
        assert_eq!(latest_save(&new_run).unwrap().slot, "c");
        assert_eq!(
            run_of_save(&slot_path(&new_run, "c")).unwrap(),
            (new_id, new_run.clone())
        );

        delete_run(data, &old_run).unwrap();
        assert!(!old_run.exists());
        // Mai fuori da runs/.
        assert!(delete_run(data, data).is_err());
        assert!(data.exists());
    }

    #[test]
    fn autosave_slots_rotate() {
        let tmp = TempDir::new("auto");
        let run = tmp.0.join("run");
        let world = World::generate(1, 2, 5);
        let brain = UtilityBrain::new(1);
        let mut used = Vec::new();
        for ms in 1..=5u64 {
            let slot = next_auto_slot(&run);
            let header = SaveHeader::of("x", &slot, ms, &world);
            let body = SaveBodyRef {
                world: &world,
                brain: &brain,
                player: [0.0; 2],
                tokens: 0,
                items: Stock::default(),
                learnt_recipes: &[],
                minutes_per_second: 1.0,
                paused: false,
            };
            write_atomic(
                &slot_path(&run, &slot),
                &encode(&header, &body).unwrap().finish(),
            )
            .unwrap();
            used.push(slot);
        }
        assert_eq!(used, ["auto-1", "auto-2", "auto-3", "auto-1", "auto-2"]);
    }

    #[test]
    fn slot_names_are_filesystem_safe() {
        assert_eq!(
            sanitize_slot("  prima  del boss "),
            Some("prima--del-boss".into())
        );
        assert_eq!(sanitize_slot("../../etc/passwd"), Some("etcpasswd".into()));
        assert_eq!(sanitize_slot("città_1"), Some("città_1".into()));
        assert_eq!(sanitize_slot(" /.. "), None);
        assert_eq!(
            sanitize_slot(&"x".repeat(100)).unwrap().len(),
            MAX_SLOT_CHARS
        );
    }

    #[test]
    fn formats_dates_and_ages() {
        assert_eq!(format_utc(0), "01/01/1970 00:00 UTC");
        assert_eq!(format_utc(1_790_604_180_000), "28/09/2026 14:03 UTC");
        assert_eq!(format_utc(951_782_400_000), "29/02/2000 00:00 UTC");
        let now = 10_000_000_000;
        assert_eq!(format_age(now, now - 5_000), "adesso");
        assert_eq!(format_age(now, now - 5 * 60_000), "5 min fa");
        assert_eq!(format_age(now, now - 3 * 3_600_000), "3 h fa");
        assert_eq!(format_age(now, now - 30 * 3_600_000), "ieri");
        assert_eq!(format_age(now, now - 4 * 86_400_000), "4 giorni fa");
    }

    #[test]
    fn game_time_label_uses_the_day_of_the_year() {
        let mut world = World::generate(1, 2, 5);
        world.clock = sim::GameTime::from_dhm(12 * 2 + 5, 14, 20);
        let header = SaveHeader::of("x", "y", 0, &world);
        assert_eq!(header.game_time_label(), "Anno 3 Giorno 5 · 14:20");
    }
}
