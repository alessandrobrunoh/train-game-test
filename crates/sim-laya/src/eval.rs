//! Valutazione dei cervelli su scenari etichettati (vedi `examples/laya_eval.rs`).
//!
//! Due insiemi di scenari, costruiti dal `sim`:
//! - [`obvious_scenarios`]: situazioni "ovvie" scritte a mano, ottenute
//!   modificando lo stato di un NPC in un mondo generato (affamato in Mensa →
//!   mangia, 23:00 stanco nel dormitorio → dorme, in turno al lavoro → lavora,
//!   solo con un amico accanto → chiacchiera, senza attrezzo al Mercato con i
//!   gettoni → compra l'attrezzo). Ogni scenario ha le risposte giuste;
//! - [`sampled_scenarios`]: decisioni vere prese durante una partita, con
//!   l'etichetta di `UtilityBrain` (misura l'accordo, non la correttezza).
//!
//! I modelli vedono, come in `LayaBrain`, solo le `top_k` opzioni migliori per
//! utilità: se la risposta giusta non è tra queste non possono indovinarla
//! (vedi [`Scenario::label_in_top`]).

pub mod deliberations;

use std::time::{Duration, Instant};

use sim::{
    Action, ActionOption, Brain, CarriageKind, DecisionRequest, GameTime, ItemKind,
    MINUTES_PER_DAY, NpcId, RandomBrain, RelationKind, StationKind, UtilityBrain, World,
};

use crate::brain::{build_query, top_k};
use crate::model::{ChoiceModel, ChoiceQuery};

/// Una decisione etichettata.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// Tipo di scenario, es. "fame in Mensa → mangia" o "partita".
    pub kind: &'static str,
    pub npc: NpcId,
    pub time: GameTime,
    /// Tutte le opzioni offerte dal `sim`, con le descrizioni.
    pub options: Vec<ActionOption>,
    /// Indici (in `options`) considerati giusti.
    pub acceptable: Vec<usize>,
    /// Le `top_k` opzioni migliori per utilità, dalla migliore.
    pub top: Vec<usize>,
    /// La domanda per il modello, sulle opzioni `top`.
    pub query: ChoiceQuery,
    /// Scelta di `UtilityBrain` (col suo rumore).
    pub utility: usize,
    /// Scelta di un secondo `UtilityBrain` con un altro seme (rumore diverso).
    pub utility_alt: usize,
    /// Scelta di `RandomBrain`.
    pub random: usize,
}

impl Scenario {
    pub fn label_in_top(&self) -> bool {
        self.top.iter().any(|j| self.acceptable.contains(j))
    }

    fn is_correct(&self, choice: usize) -> bool {
        self.acceptable.contains(&choice)
    }
}

/// I cervelli di riferimento che decidono mentre si costruiscono gli scenari.
struct Baselines {
    utility: UtilityBrain,
    utility_alt: UtilityBrain,
    random: RandomBrain,
}

impl Baselines {
    fn new(seed: u64) -> Self {
        Self {
            utility: UtilityBrain::new(seed ^ 0x5eed),
            utility_alt: UtilityBrain::new(seed ^ 0xa17),
            random: RandomBrain::new(seed ^ 0x7a2d),
        }
    }

    fn scenario(
        &mut self,
        world: &World,
        kind: &'static str,
        request: DecisionRequest,
        utility: Option<usize>,
        acceptable: Vec<usize>,
        k: usize,
    ) -> Scenario {
        let reqs = std::slice::from_ref(&request);
        let scores = self.utility.scores(world, &request);
        let top = top_k(&scores, k);
        let query = build_query(world, &request, &top);
        let utility = utility.unwrap_or_else(|| self.utility.decide(world, reqs)[0]);
        let utility_alt = self.utility_alt.decide(world, reqs)[0];
        let random = self.random.decide(world, reqs)[0];
        Scenario {
            kind,
            npc: request.npc,
            time: world.clock,
            options: request.options,
            acceptable,
            top,
            query,
            utility,
            utility_alt,
            random,
        }
    }
}

/// Tipi di scenario ovvio.
pub const OBVIOUS_KINDS: [&str; 5] = [
    "fame in Mensa → mangia",
    "23:00 stanco a casa → dorme",
    "in turno al lavoro → lavora",
    "solo, amico accanto → chiacchiera",
    "senza attrezzo al Mercato → compra",
];

/// Bisogni tranquilli e niente da comprare: solo quello che lo scenario cambia conta.
fn calm(world: &mut World, id: NpcId) {
    let Some(npc) = world.npcs.iter_mut().find(|n| n.id == id) else {
        return;
    };
    npc.needs.hunger = 0.85;
    npc.needs.energy = 0.85;
    npc.needs.social = 0.85;
    npc.inventory.clothes = Some(1.0);
    if npc.job.is_some_and(|j| j.uses_tool()) {
        npc.inventory.tool = Some(1.0);
    }
    npc.action = Action::Idle;
}

/// Libera tutte le postazioni di un tipo in una carrozza.
fn free_stations(world: &mut World, carriage: sim::CarriageId, kind: StationKind) {
    if let Some(c) = world.carriages.get_mut(carriage.index()) {
        for s in c.stations.iter_mut().filter(|s| s.kind == kind) {
            s.occupancy = 0;
        }
    }
}

fn npc_mut(world: &mut World, id: NpcId) -> &mut sim::Npc {
    world
        .npcs
        .iter_mut()
        .find(|n| n.id == id)
        .expect("NPC dello scenario")
}

fn first_of(world: &World, kind: CarriageKind) -> Option<sim::CarriageId> {
    world
        .carriages
        .iter()
        .find(|c| c.kind == kind)
        .map(|c| c.id)
}

/// Mondo di partenza: generato e fatto vivere due giorni (nascono amicizie).
pub fn base_world(seed: u64) -> World {
    let mut world = World::generate(seed, 20, 300);
    world.run(&mut UtilityBrain::new(seed), 2 * MINUTES_PER_DAY);
    world
}

/// `per_kind` scenari ovvi per ogni tipo di [`OBVIOUS_KINDS`], con `k` opzioni
/// per il modello. Deterministico dato il seme.
pub fn obvious_scenarios(seed: u64, per_kind: usize, k: usize) -> Vec<Scenario> {
    let base = base_world(seed);
    let mut baselines = Baselines::new(seed);
    let day = base.clock.day() + 1;
    let adults: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.stage().works())
        .map(|n| n.id)
        .collect();
    let workers: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.job.is_some() && n.workplace.is_some())
        .map(|n| n.id)
        .collect();
    let tool_users: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.job.is_some_and(|j| j.uses_tool()))
        .map(|n| n.id)
        .collect();
    // Coppie (NPC, legame caro) con entrambi vivi.
    let pairs: Vec<(NpcId, NpcId)> = base
        .npcs
        .iter()
        .filter(|n| n.stage().works())
        .filter_map(|n| {
            n.relations
                .iter()
                .find(|r| {
                    base.npc(r.other).is_some()
                        && (r.kind != RelationKind::Friend || r.affinity > 0.0)
                })
                .map(|r| (n.id, r.other))
        })
        .collect();
    let mensa = first_of(&base, CarriageKind::Mensa);
    let mercato = first_of(&base, CarriageKind::Mercato);
    let meal_times = [(12, 20), (19, 10), (13, 0), (20, 5), (12, 45), (19, 40)];

    let mut out = Vec::new();
    for v in 0..per_kind {
        // 1. Affamato in una Mensa con cibo.
        if let (Some(mensa), Some(&id)) = (mensa, adults.get((v * 7) % adults.len().max(1))) {
            let mut w = base.clone();
            let (h, m) = meal_times[v % meal_times.len()];
            w.clock = GameTime::from_dhm(day, h, m);
            calm(&mut w, id);
            let cap = w.params.storage_cap(CarriageKind::Mensa, ItemKind::Razione);
            w.carriages[mensa.index()].stock.set(ItemKind::Razione, cap);
            free_stations(&mut w, mensa, StationKind::Table);
            let npc = npc_mut(&mut w, id);
            npc.carriage = mensa;
            npc.needs.hunger = 0.03 + 0.02 * (v % 3) as f32;
            push(
                &mut out,
                &mut baselines,
                &mut w,
                OBVIOUS_KINDS[0],
                id,
                k,
                |o| matches!(o.action, Action::Eat(_)),
            );
        }
        // 2. Alle 23, sfinito, nel proprio dormitorio.
        if let Some(&id) = adults.get((v * 11 + 3) % adults.len().max(1)) {
            let mut w = base.clone();
            w.clock = GameTime::from_dhm(day, 23, (v as u64 * 4) % 50);
            calm(&mut w, id);
            let home = npc_mut(&mut w, id).home;
            free_stations(&mut w, home, StationKind::Bed);
            let npc = npc_mut(&mut w, id);
            npc.carriage = home;
            npc.needs.energy = 0.05 + 0.03 * (v % 3) as f32;
            push(
                &mut out,
                &mut baselines,
                &mut w,
                OBVIOUS_KINDS[1],
                id,
                k,
                |o| matches!(o.action, Action::Sleep(_)),
            );
        }
        // 3. Alle 10 in turno, sul posto di lavoro, bisogni a posto.
        if let Some(&id) = workers.get((v * 5 + 1) % workers.len().max(1)) {
            let mut w = base.clone();
            w.clock = GameTime::from_dhm(day, 10, (v as u64 * 6) % 60);
            calm(&mut w, id);
            let npc = npc_mut(&mut w, id);
            let (job, place) = (npc.job.expect("lavoratore"), npc.workplace.expect("posto"));
            npc.carriage = place;
            free_stations(&mut w, place, job.station_kind());
            push(
                &mut out,
                &mut baselines,
                &mut w,
                OBVIOUS_KINDS[2],
                id,
                k,
                |o| matches!(o.action, Action::Work(_)),
            );
        }
        // 4. Solo, con un amico o parente disponibile accanto (la sera).
        if let Some(&(id, friend)) = pairs.get((v * 13 + 2) % pairs.len().max(1)) {
            let mut w = base.clone();
            w.clock = GameTime::from_dhm(day, 18, 30 + (v as u64 * 3) % 25);
            calm(&mut w, id);
            calm(&mut w, friend);
            let here = npc_mut(&mut w, id).home;
            let npc = npc_mut(&mut w, id);
            npc.carriage = here;
            npc.floor = 0;
            npc.needs.social = 0.05 + 0.03 * (v % 3) as f32;
            let friend = npc_mut(&mut w, friend);
            // Accanto: stessa carrozza e stesso piano.
            friend.carriage = here;
            friend.floor = 0;
            push(
                &mut out,
                &mut baselines,
                &mut w,
                OBVIOUS_KINDS[3],
                id,
                k,
                |o| matches!(o.action, Action::Socialize(_)),
            );
        }
        // 5. Lavoratore senza attrezzo, con i gettoni, in un Mercato fornito.
        if let (Some(mercato), Some(&id)) = (
            mercato,
            tool_users.get((v * 3 + 1) % tool_users.len().max(1)),
        ) {
            let mut w = base.clone();
            w.clock = GameTime::from_dhm(day, 18, 10 + (v as u64 * 5) % 40);
            calm(&mut w, id);
            let cap = w
                .params
                .storage_cap(CarriageKind::Mercato, ItemKind::Attrezzo);
            w.carriages[mercato.index()]
                .stock
                .set(ItemKind::Attrezzo, cap);
            let npc = npc_mut(&mut w, id);
            npc.carriage = mercato;
            npc.inventory.tool = None;
            npc.inventory.tokens = 200;
            push(
                &mut out,
                &mut baselines,
                &mut w,
                OBVIOUS_KINDS[4],
                id,
                k,
                |o| o.action == Action::Buy(ItemKind::Attrezzo),
            );
        }
    }
    out
}

/// Aggiunge uno scenario se tra le opzioni c'è almeno una risposta giusta.
fn push(
    out: &mut Vec<Scenario>,
    baselines: &mut Baselines,
    world: &mut World,
    kind: &'static str,
    id: NpcId,
    k: usize,
    good: impl Fn(&ActionOption) -> bool,
) {
    let options = world.options(id);
    let acceptable: Vec<usize> = (0..options.len()).filter(|&j| good(&options[j])).collect();
    if acceptable.is_empty() || options.len() < 2 {
        return;
    }
    let request = DecisionRequest { npc: id, options };
    out.push(baselines.scenario(world, kind, request, None, acceptable, k));
}

/// Decisioni vere di una partita (dopo un giorno di rodaggio), una ogni
/// `every`, etichettate da `UtilityBrain`; restituisce anche quante decisioni
/// al secondo ha preso `UtilityBrain` nel frattempo.
pub fn sampled_scenarios(seed: u64, n: usize, every: usize, k: usize) -> (Vec<Scenario>, f64) {
    struct Recorder {
        inner: UtilityBrain,
        baselines: Baselines,
        every: usize,
        seen: usize,
        n: usize,
        k: usize,
        out: Vec<Scenario>,
        decide_time: Duration,
        decisions: usize,
    }
    impl Brain for Recorder {
        fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
            let start = Instant::now();
            let choices = self.inner.decide(world, requests);
            self.decide_time += start.elapsed();
            self.decisions += requests.len();
            for (r, &c) in requests.iter().zip(&choices) {
                self.seen += 1;
                if self.out.len() < self.n
                    && r.options.len() >= 2
                    && self.seen.is_multiple_of(self.every)
                {
                    let s = self.baselines.scenario(
                        world,
                        "partita",
                        r.clone(),
                        Some(c),
                        vec![c],
                        self.k,
                    );
                    self.out.push(s);
                }
            }
            choices
        }
    }

    let mut world = World::generate(seed, 20, 300);
    let mut warm = UtilityBrain::new(seed);
    world.run(&mut warm, MINUTES_PER_DAY);
    let mut rec = Recorder {
        inner: warm,
        baselines: Baselines::new(seed),
        every: every.max(1),
        seen: 0,
        n,
        k,
        out: Vec::new(),
        decide_time: Duration::ZERO,
        decisions: 0,
    };
    for _ in 0..30 {
        if rec.out.len() >= n {
            break;
        }
        world.run(&mut rec, MINUTES_PER_DAY / 4);
    }
    let rate = rec.decisions as f64 / rec.decide_time.as_secs_f64().max(1e-9);
    (rec.out, rate)
}

/// Risultati di un cervello o modello.
#[derive(Clone, Debug, Default)]
pub struct EvalRow {
    pub name: String,
    /// Accuratezza sugli scenari ovvi, e per tipo.
    pub accuracy: Option<f32>,
    pub accuracy_by_kind: Vec<(&'static str, f32)>,
    /// Accordo con `UtilityBrain` sulle decisioni della partita.
    pub agreement: Option<f32>,
    /// Istogramma della confidenza (5 fasce da 0.2), solo per i modelli.
    pub confidence_hist: Option<[usize; 5]>,
    pub mean_confidence: Option<f32>,
    /// Decisioni al secondo.
    pub throughput: Option<f64>,
    pub error: Option<String>,
}

fn accuracy_of(
    scenarios: &[Scenario],
    choices: &[usize],
) -> (Option<f32>, Vec<(&'static str, f32)>) {
    let rate = |pairs: Vec<(&Scenario, usize)>| {
        (!pairs.is_empty()).then(|| {
            pairs.iter().filter(|(s, c)| s.is_correct(*c)).count() as f32 / pairs.len() as f32
        })
    };
    let all = rate(scenarios.iter().zip(choices.iter().copied()).collect());
    let by_kind = OBVIOUS_KINDS
        .iter()
        .filter_map(|&kind| {
            let pairs: Vec<_> = scenarios
                .iter()
                .zip(choices.iter().copied())
                .filter(|(s, _)| s.kind == kind)
                .collect();
            rate(pairs).map(|r| (kind, r))
        })
        .collect();
    (all, by_kind)
}

/// Riga per un cervello che ha già deciso (le scelte sono negli scenari).
pub fn baseline_row(
    name: &str,
    obvious: &[Scenario],
    sampled: &[Scenario],
    pick: impl Fn(&Scenario) -> usize,
    throughput: Option<f64>,
) -> EvalRow {
    let a: Vec<usize> = obvious.iter().map(&pick).collect();
    let b: Vec<usize> = sampled.iter().map(&pick).collect();
    let (accuracy, accuracy_by_kind) = accuracy_of(obvious, &a);
    EvalRow {
        name: name.to_string(),
        accuracy,
        accuracy_by_kind,
        agreement: accuracy_of(sampled, &b).0,
        throughput,
        ..EvalRow::default()
    }
}

/// Valuta un modello sulle domande top-k degli scenari, a lotti di `batch`.
pub fn model_row(
    name: &str,
    model: &mut dyn ChoiceModel,
    obvious: &[Scenario],
    sampled: &[Scenario],
    batch: usize,
) -> EvalRow {
    let mut row = EvalRow {
        name: name.to_string(),
        ..EvalRow::default()
    };
    let mut hist = [0usize; 5];
    let mut conf_sum = 0.0f64;
    let mut answered = 0usize;
    let start = Instant::now();
    let mut run = |scenarios: &[Scenario]| -> Result<Vec<usize>, String> {
        let mut choices = Vec::with_capacity(scenarios.len());
        for chunk in scenarios.chunks(batch.max(1)) {
            let queries: Vec<ChoiceQuery> = chunk.iter().map(|s| s.query.clone()).collect();
            let answers = model.predict_batch(&queries)?;
            if answers.len() != chunk.len() {
                return Err(format!(
                    "{} risposte per {} domande",
                    answers.len(),
                    chunk.len()
                ));
            }
            for (s, a) in chunk.iter().zip(answers) {
                let (best, p) = a.best().ok_or("risposta vuota")?;
                hist[((p * 5.0) as usize).min(4)] += 1;
                conf_sum += f64::from(p);
                answered += 1;
                choices.push(s.top.get(best).copied().unwrap_or(0));
            }
        }
        Ok(choices)
    };
    let result = run(obvious).and_then(|a| run(sampled).map(|b| (a, b)));
    let elapsed = start.elapsed().as_secs_f64();
    match result {
        Ok((a, b)) => {
            let (accuracy, by_kind) = accuracy_of(obvious, &a);
            row.accuracy = accuracy;
            row.accuracy_by_kind = by_kind;
            row.agreement = accuracy_of(sampled, &b).0;
            row.confidence_hist = Some(hist);
            row.mean_confidence = (answered > 0).then(|| (conf_sum / answered as f64) as f32);
            row.throughput = Some(answered as f64 / elapsed.max(1e-9));
        }
        Err(e) => row.error = Some(e),
    }
    row
}

/// Tabella compatta dei risultati.
pub fn format_table(rows: &[EvalRow]) -> String {
    let pct = |v: Option<f32>| v.map_or("—".to_string(), |v| format!("{:.0}%", v * 100.0));
    let mut out = format!(
        "{:<28} {:>9} {:>11} {:>8} {:>22} {:>12}\n",
        "cervello", "ovvi (a)", "accordo (b)", "conf.", "conf. 0-.2-.4-.6-.8-1", "decisioni/s"
    );
    for r in rows {
        if let Some(e) = &r.error {
            out.push_str(&format!("{:<28} errore: {e}\n", r.name));
            continue;
        }
        let hist = r.confidence_hist.map_or("—".to_string(), |h| {
            h.iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("/")
        });
        let tp = r.throughput.map_or("—".to_string(), |t| format!("{t:.0}"));
        out.push_str(&format!(
            "{:<28} {:>9} {:>11} {:>8} {:>22} {:>12}\n",
            r.name,
            pct(r.accuracy),
            pct(r.agreement),
            r.mean_confidence
                .map_or("—".to_string(), |c| format!("{c:.2}")),
            hist,
            tp
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockModel;

    #[test]
    fn obvious_scenarios_cover_every_kind_and_are_solvable() {
        let scenarios = obvious_scenarios(3, 2, 5);
        for kind in OBVIOUS_KINDS {
            assert!(
                scenarios.iter().any(|s| s.kind == kind),
                "nessuno scenario per {kind}"
            );
        }
        for s in &scenarios {
            assert!(!s.acceptable.is_empty());
            assert_eq!(s.query.options.len(), s.top.len());
            assert!(s.top.len() <= 5 && s.top.len() >= 2);
        }
        // Il mock e l'utilità risolvono gli scenari ovvi quasi sempre.
        let mock = model_row("mock", &mut MockModel::new(), &scenarios, &[], 16);
        assert!(mock.accuracy.unwrap() >= 0.8, "{mock:?}");
        let utility = baseline_row("utility", &scenarios, &[], |s| s.utility, None);
        assert!(utility.accuracy.unwrap() >= 0.8, "{utility:?}");
    }

    #[test]
    fn sampled_scenarios_are_labeled_by_utility() {
        let (scenarios, rate) = sampled_scenarios(5, 40, 7, 5);
        assert_eq!(scenarios.len(), 40);
        assert!(rate > 0.0);
        for s in &scenarios {
            assert_eq!(s.acceptable, vec![s.utility]);
            assert!(s.utility < s.options.len());
        }
        let table = format_table(&[baseline_row(
            "utility",
            &[],
            &scenarios,
            |s| s.utility,
            Some(rate),
        )]);
        assert!(table.contains("utility") && table.contains("100%"));
    }
}
