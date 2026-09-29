//! Valutazione sulle deliberazioni (vedi `examples/laya_eval.rs`).
//!
//! Due insiemi, costruiti dal `sim`:
//! - [`obvious_deliberations`]: casi "ovvi" scritti a mano, per ogni tipo
//!   di deliberazione, ottenuti modificando un mondo generato (affinità,
//!   età, carattere, figli, razioni, gettoni) e aprendo la deliberazione con
//!   `World::open_deliberation` (es. affinità altissima, coetanei, single →
//!   accetta; disperato, audace e disonesto → ruba). Ogni caso ha le risposte giuste;
//! - [`sampled_deliberations`]: deliberazioni vere di una lunga partita,
//!   etichettate con la scelta più probabile della regola del `sim` (misura
//!   l'accordo con la regola, non la correttezza).
//!
//! I modelli si chiamano una volta sola per scenario ([`predict_all`]); le
//! righe con la miscela della regola (`prior_weight`) e le soglie si
//! ricavano poi dalle stesse probabilità.

use std::time::Instant;

use sim::{
    Brain, CarriageId, CarriageKind, Choice, DecisionRequest, Deliberation, DeliberationKind,
    Grievance, ItemKind, LifeStage, MINUTES_PER_DAY, Npc, NpcId, Relation, RelationKind, Sex,
    UtilityBrain, World,
};

use crate::deliberation::{KINDS, argmax, blend, deliberation_query};
use crate::model::{ChoiceModel, ChoiceQuery};

/// Una deliberazione etichettata.
#[derive(Clone, Debug)]
pub struct DelibScenario {
    /// Tipo di caso ovvio (vedi [`OBVIOUS_DELIBERATIONS`]) o "partita".
    pub case: &'static str,
    pub deliberation: Deliberation,
    /// Probabilità della regola del `sim` all'apertura.
    pub rule: Vec<f32>,
    /// Indici delle opzioni giuste.
    pub acceptable: Vec<usize>,
    pub query: ChoiceQuery,
    /// Un'estrazione dalla regola (come fa il `sim`).
    pub rule_sampled: usize,
    /// Un'opzione a caso.
    pub random: usize,
}

impl DelibScenario {
    pub fn topic(&self) -> usize {
        self.deliberation.kind.index()
    }

    pub fn rule_argmax(&self) -> usize {
        argmax(&self.rule).map_or(0, |(i, _)| i)
    }

    pub fn is_correct(&self, choice: usize) -> bool {
        self.acceptable.contains(&choice)
    }
}

/// Tipi di caso ovvio.
pub const OBVIOUS_DELIBERATIONS: [&str; 9] = [
    "coppia: affini, coetanei → accetta",
    "coppia: poca affinità, 25+ anni → rifiuta",
    "figlio: giovani, cibo abbondante → prova",
    "figlio: 4 figli, poco cibo → aspetta",
    "furto: onesto, quasi basta → risparmia",
    "furto: disperato, audace, disonesto → ruba",
    "furto: famiglia affettuosa → chiede aiuto",
    "protesta: audace senza figli → protesta",
    "protesta: prudente con figli → sopporta",
];

/// Numero pseudo-casuale in `0..1` da una chiave (splitmix64).
fn unit(key: u64) -> f32 {
    let mut z = key.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

fn sample(weights: &[f32], roll: f32) -> usize {
    let mut acc = 0.0;
    for (k, w) in weights.iter().enumerate() {
        acc += w;
        if roll < acc {
            return k;
        }
    }
    weights.len().saturating_sub(1)
}

fn scenario(
    case: &'static str,
    d: Deliberation,
    rule: Vec<f32>,
    acceptable: Vec<usize>,
    salt: u64,
) -> DelibScenario {
    let key = d.id.0.wrapping_mul(31) ^ salt.wrapping_mul(0x51);
    let rule_sampled = sample(&rule, unit(key));
    let random = ((unit(key ^ 0xabcd) * d.options.len() as f32) as usize)
        .min(d.options.len().saturating_sub(1));
    DelibScenario {
        case,
        query: deliberation_query(&d),
        deliberation: d,
        rule,
        acceptable,
        rule_sampled,
        random,
    }
}

// --- Casi ovvi -------------------------------------------------------------------

fn index(w: &World, id: NpcId) -> usize {
    w.npcs.iter().position(|n| n.id == id).expect("NPC")
}

fn npc_mut(w: &mut World, id: NpcId) -> &mut Npc {
    let i = index(w, id);
    &mut w.npcs[i]
}

/// Legame `kind` (visto da `a`) con `affinity`, nei due sensi.
fn bond(w: &mut World, a: NpcId, b: NpcId, kind: RelationKind, affinity: f32) {
    for (x, y, k) in [(a, b, kind), (b, a, kind.inverse())] {
        let npc = npc_mut(w, x);
        npc.relations.retain(|r| r.other != y);
        npc.relations.push(Relation {
            other: y,
            kind: k,
            affinity,
        });
    }
}

fn single_adult(n: &Npc, sex: Sex) -> bool {
    n.sex == sex && n.stage() == LifeStage::Adulto && n.partner().is_none()
}

fn childless(n: &Npc) -> bool {
    n.children().next().is_none()
}

/// Razioni nelle Mense: `per_person` a testa.
fn set_food(w: &mut World, per_person: f32) {
    let mense: Vec<usize> = w
        .carriages
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == CarriageKind::Mensa)
        .map(|(i, _)| i)
        .collect();
    let total = per_person * w.npcs.len() as f32;
    let each = total / mense.len().max(1) as f32;
    for i in mense {
        w.carriages[i].stock.set(ItemKind::Razione, each);
    }
}

fn first_of(w: &World, kind: CarriageKind) -> Option<CarriageId> {
    w.carriages.iter().find(|c| c.kind == kind).map(|c| c.id)
}

/// Apre la deliberazione e la trasforma in scenario (None se non si può descrivere).
fn open(
    w: &mut World,
    case: &'static str,
    npc: NpcId,
    kind: DeliberationKind,
    good: &[Choice],
    salt: u64,
) -> Option<DelibScenario> {
    let id = w.open_deliberation(npc, kind)?;
    let d = w.deliberation(id)?.clone();
    let rule = w.deliberation_rule_weights(id)?;
    let acceptable: Vec<usize> = good.iter().filter_map(|&c| d.option_of(c)).collect();
    (!acceptable.is_empty()).then(|| scenario(case, d, rule, acceptable, salt))
}

/// Il `v`-esimo elemento (ciclico) di una lista, se non è vuota.
fn pick<T: Copy>(list: &[T], v: usize, stride: usize) -> Option<T> {
    (!list.is_empty()).then(|| list[(v * stride) % list.len()])
}

/// `per_kind` casi ovvi per ogni tipo di [`OBVIOUS_DELIBERATIONS`] (meno se
/// il mondo non ha abbastanza NPC adatti). Deterministico dato il seme.
pub fn obvious_deliberations(seed: u64, per_kind: usize) -> Vec<DelibScenario> {
    let base = crate::eval::base_world(seed);
    let mut out = Vec::new();
    let cases = &OBVIOUS_DELIBERATIONS;
    let mensa = first_of(&base, CarriageKind::Mensa).unwrap_or(CarriageId(0));
    let market = first_of(&base, CarriageKind::Mercato);

    // Coppie possibili: donna single e uomo single.
    let women: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| single_adult(n, Sex::Female) && childless(n) && n.age <= 40)
        .map(|n| n.id)
        .collect();
    let men: Vec<&Npc> = base
        .npcs
        .iter()
        .filter(|n| single_adult(n, Sex::Male) && childless(n))
        .collect();
    let closest_man = |age: u32, min_gap: u32, max_gap: u32| -> Option<NpcId> {
        men.iter()
            .filter(|m| (min_gap..=max_gap).contains(&m.age.abs_diff(age)))
            .min_by_key(|m| (m.age.abs_diff(age), m.id))
            .map(|m| m.id)
    };
    // Donne in coppia in età fertile.
    let mothers: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.sex == Sex::Female && n.partner().is_some() && n.age >= 20 && n.age <= 40)
        .map(|n| n.id)
        .collect();
    let kids: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.age < LifeStage::ADULTO_FROM)
        .map(|n| n.id)
        .collect();
    let tool_users: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.job.is_some_and(|j| j.uses_tool()))
        .map(|n| n.id)
        .collect();
    let tool_users_in_couple: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.job.is_some_and(|j| j.uses_tool()) && n.partner().is_some())
        .map(|n| n.id)
        .collect();
    let in_couple_childless: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.partner().is_some() && childless(n) && n.age <= 45)
        .map(|n| n.id)
        .collect();
    let parents: Vec<NpcId> = base
        .npcs
        .iter()
        .filter(|n| n.stage().works() && !childless(n) && n.job.is_some())
        .map(|n| n.id)
        .collect();

    for v in 0..per_kind {
        let salt = seed ^ ((v as u64) << 8);
        // 1. Affinità altissima, coetanei, entrambi single → accetta.
        if let Some(a) = pick(&women, v, 7) {
            let age = base.npc(a).map_or(30, |n| n.age);
            if let Some(b) = closest_man(age, 0, 2) {
                let mut w = base.clone();
                bond(&mut w, a, b, RelationKind::Friend, 0.97);
                npc_mut(&mut w, a).traits.boldness = 0.6;
                out.extend(open(
                    &mut w,
                    cases[0],
                    a,
                    DeliberationKind::CoupleProposal { from: b },
                    &[Choice::Accept],
                    salt,
                ));
            }
        }
        // 2. Poca affinità e più di 25 anni di differenza → rifiuta.
        if let Some(a) = pick(&women, v, 5) {
            let age = base.npc(a).map_or(25, |n| n.age);
            if let Some(b) = closest_man(age, 25, 80) {
                let mut w = base.clone();
                bond(&mut w, a, b, RelationKind::Friend, 0.1);
                npc_mut(&mut w, a).traits.boldness = 0.3;
                out.extend(open(
                    &mut w,
                    cases[1],
                    a,
                    DeliberationKind::CoupleProposal { from: b },
                    &[Choice::Refuse],
                    salt,
                ));
            }
        }
        // 3. Giovani, senza figli, molto uniti, cibo abbondante → prova.
        if let Some(m) = pick(&mothers, v, 3) {
            let mut w = base.clone();
            let partner = w.npc(m).and_then(Npc::partner).expect("partner");
            npc_mut(&mut w, m)
                .relations
                .retain(|r| r.kind != RelationKind::Child);
            let n = npc_mut(&mut w, m);
            n.age = 24 + (v as u32 % 5);
            n.inventory.tokens = 40;
            bond(&mut w, m, partner, RelationKind::Partner, 0.95);
            npc_mut(&mut w, partner).inventory.tokens = 40;
            set_food(&mut w, 4.0);
            out.extend(open(
                &mut w,
                cases[2],
                m,
                DeliberationKind::HaveChild { partner },
                &[Choice::TryForChild],
                salt,
            ));
        }
        // 4. Già 4 figli, poche razioni, pochi gettoni → aspetta.
        if let Some(m) = pick(&mothers, v, 5)
            && kids.len() >= 4
        {
            let mut w = base.clone();
            let partner = w.npc(m).and_then(Npc::partner).expect("partner");
            let n = npc_mut(&mut w, m);
            n.relations.retain(|r| r.kind != RelationKind::Child);
            for k in 0..4 {
                n.relations.push(Relation {
                    other: kids[(v * 4 + k) % kids.len()],
                    kind: RelationKind::Child,
                    affinity: 0.8,
                });
            }
            n.age = 38;
            n.inventory.tokens = 4;
            npc_mut(&mut w, partner).inventory.tokens = 4;
            set_food(&mut w, 0.3);
            out.extend(open(
                &mut w,
                cases[3],
                m,
                DeliberationKind::HaveChild { partner },
                &[Choice::Wait],
                salt,
            ));
        }
        // Furti: un attrezzo al primo Mercato, ben fornito.
        let theft = |w: &mut World, id: NpcId, tokens_short: Option<u32>| -> Option<CarriageId> {
            let market = market?;
            let cap = w.storage_cap(CarriageKind::Mercato, ItemKind::Attrezzo);
            w.carriages[market.index()]
                .stock
                .set(ItemKind::Attrezzo, cap);
            let price = w.price(market, ItemKind::Attrezzo)?;
            let n = npc_mut(w, id);
            n.inventory.tool = None;
            n.carriage = market;
            n.inventory.tokens = tokens_short.map_or(0, |s| price.saturating_sub(s));
            Some(market)
        };
        // 5. Onesto e prudente, gli manca un gettone → risparmia.
        if let Some(id) = pick(&tool_users, v, 7) {
            let mut w = base.clone();
            if let Some(market) = theft(&mut w, id, Some(1)) {
                let n = npc_mut(&mut w, id);
                n.traits.honesty = 0.95;
                n.traits.boldness = 0.3;
                out.extend(open(
                    &mut w,
                    cases[4],
                    id,
                    DeliberationKind::Theft {
                        item: ItemKind::Attrezzo,
                        market,
                        helper: None,
                    },
                    &[Choice::Save],
                    salt,
                ));
            }
        }
        // 6. Nessun gettone, audace e disonesto → ruba.
        if let Some(id) = pick(&tool_users, v, 11) {
            let mut w = base.clone();
            if let Some(market) = theft(&mut w, id, None) {
                let n = npc_mut(&mut w, id);
                n.traits.honesty = 0.05;
                n.traits.boldness = 0.95;
                out.extend(open(
                    &mut w,
                    cases[5],
                    id,
                    DeliberationKind::Theft {
                        item: ItemKind::Attrezzo,
                        market,
                        helper: None,
                    },
                    &[Choice::Steal],
                    salt,
                ));
            }
        }
        // 7. Il partner lo adora e ha molti gettoni → chiede aiuto.
        if let Some(id) = pick(&tool_users_in_couple, v, 3) {
            let mut w = base.clone();
            let helper = w.npc(id).and_then(Npc::partner).expect("partner");
            if let Some(market) = theft(&mut w, id, None) {
                let n = npc_mut(&mut w, id);
                n.inventory.tokens = 2;
                n.traits.honesty = 0.7;
                n.traits.boldness = 0.3;
                bond(&mut w, id, helper, RelationKind::Partner, 1.0);
                npc_mut(&mut w, helper).inventory.tokens = 120;
                out.extend(open(
                    &mut w,
                    cases[6],
                    id,
                    DeliberationKind::Theft {
                        item: ItemKind::Attrezzo,
                        market,
                        helper: Some(helper),
                    },
                    &[Choice::AskForHelp],
                    salt,
                ));
            }
        }
        // 8. Audace, in coppia senza figli, nascita negata → protesta.
        if let Some(id) = pick(&in_couple_childless, v, 5) {
            let mut w = base.clone();
            npc_mut(&mut w, id).traits.boldness = 0.95;
            out.extend(open(
                &mut w,
                cases[7],
                id,
                DeliberationKind::Protest {
                    grievance: Grievance::BirthDenied,
                    place: mensa,
                },
                &[Choice::Protest],
                salt,
            ));
        }
        // 9. Prudente, ha già figli, nascita negata → sopporta (o lavora di più).
        if let Some(id) = pick(&parents, v, 7) {
            let mut w = base.clone();
            npc_mut(&mut w, id).traits.boldness = 0.05;
            out.extend(open(
                &mut w,
                cases[8],
                id,
                DeliberationKind::Protest {
                    grievance: Grievance::BirthDenied,
                    place: mensa,
                },
                &[Choice::Endure, Choice::WorkHarder],
                salt,
            ));
        }
    }
    out
}

// --- Deliberazioni della partita ---------------------------------------------------

/// `n` deliberazioni di partite vere (400 NPC, deliberazioni tre volte più
/// frequenti del normale e proteste più probabili), al più `n / 4` per tipo
/// finché ce ne sono, etichettate con la scelta più probabile della regola
/// quando si sono aperte.
pub fn sampled_deliberations(seed: u64, n: usize) -> Vec<DelibScenario> {
    struct Capture {
        inner: UtilityBrain,
        seed: u64,
        by_kind: [Vec<DelibScenario>; KINDS],
    }
    impl Brain for Capture {
        fn decide(&mut self, world: &World, requests: &[DecisionRequest]) -> Vec<usize> {
            self.inner.decide(world, requests)
        }
        fn wants_descriptions(&self) -> bool {
            false
        }
        fn deliberations_opened(&mut self, world: &World, new: &[Deliberation]) {
            for d in new {
                let Some(rule) = world.deliberation_rule_weights(d.id) else {
                    continue;
                };
                let label = argmax(&rule).map_or(0, |(i, _)| i);
                self.by_kind[d.kind.index()].push(scenario(
                    "partita",
                    d.clone(),
                    rule,
                    vec![label],
                    self.seed,
                ));
            }
        }
    }
    let mut brain = Capture {
        inner: UtilityBrain::new(seed),
        seed,
        by_kind: Default::default(),
    };
    let quota = n.div_ceil(KINDS);
    // Due partite: una normale, poi (se servono proteste) una in cui i
    // dormitori sono "pieni" prima, con più nascite negate e quindi proteste.
    for crowded in [false, true] {
        let mut world = World::generate(seed + u64::from(crowded), 20, 400);
        world.params.deliberation_rate = 3.0;
        world.params.protest_chance_on_denial = 1.0;
        world.params.protest_chance_on_shortage = 0.5;
        if crowded {
            world.params.birth_max_bed_occupancy = 0.6;
        }
        for _ in 0..30 {
            let full = brain.by_kind.iter().all(|b| b.len() >= quota);
            let protests = brain.by_kind[3].len() >= quota;
            if full || (crowded && protests) {
                break;
            }
            world.run(&mut brain, MINUTES_PER_DAY);
        }
    }
    // Prima fino alla quota per tipo, poi (se un tipo è raro) il resto in ordine.
    let mut out: Vec<DelibScenario> = Vec::with_capacity(n);
    let mut rest = Vec::new();
    for bucket in brain.by_kind {
        let mut bucket = bucket.into_iter();
        out.extend(bucket.by_ref().take(quota));
        rest.extend(bucket);
    }
    rest.sort_by_key(|s| s.deliberation.id);
    let missing = n.saturating_sub(out.len());
    out.extend(rest.into_iter().take(missing));
    out.truncate(n);
    out.sort_by_key(|s| s.deliberation.id);
    out
}

// --- Modelli e tabelle -------------------------------------------------------------

/// Le probabilità di un modello per ogni scenario (una chiamata sola per
/// scenario, a lotti di `batch`), e le righe al secondo.
pub fn predict_all(
    model: &mut dyn ChoiceModel,
    scenarios: &[DelibScenario],
    batch: usize,
) -> Result<(Vec<Vec<f32>>, f64), String> {
    let start = Instant::now();
    let mut out = Vec::with_capacity(scenarios.len());
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
            if a.probabilities.len() != s.query.options.len() {
                return Err("probabilità e opzioni non coincidono".into());
            }
            out.push(a.probabilities);
        }
    }
    let rate = out.len() as f64 / start.elapsed().as_secs_f64().max(1e-9);
    Ok((out, rate))
}

/// Soglie mostrate nella tabella della copertura.
pub const THRESHOLDS: [f32; 4] = [0.5, 0.6, 0.7, 0.8];

/// Risultati di un cervello sulle deliberazioni.
#[derive(Clone, Debug, Default)]
pub struct DelibRow {
    pub name: String,
    /// Accuratezza sui casi ovvi (a), e per caso.
    pub accuracy: Option<f32>,
    pub accuracy_by_case: Vec<(&'static str, f32)>,
    /// Accordo con la regola sulle deliberazioni della partita (b), e per tipo.
    pub agreement: Option<f32>,
    pub agreement_by_kind: [Option<f32>; KINDS],
    /// Solo per i modelli: confidenza media e istogramma (5 fasce da 0.2) su (a)+(b).
    pub mean_confidence: Option<f32>,
    pub confidence_hist: Option<[usize; 5]>,
    /// Per ogni soglia di [`THRESHOLDS`]: frazione di risposte sopra soglia
    /// (a+b) e accuratezza di quelle su (a) e accordo su (b).
    pub coverage: Vec<(f32, f32, Option<f32>, Option<f32>)>,
    pub throughput: Option<f64>,
    pub error: Option<String>,
}

fn rate(pairs: impl Iterator<Item = bool>) -> Option<f32> {
    let (mut ok, mut n) = (0usize, 0usize);
    for good in pairs {
        n += 1;
        ok += usize::from(good);
    }
    (n > 0).then(|| ok as f32 / n as f32)
}

/// Riga per un cervello che sceglie senza confidenza (regola, caso).
pub fn delib_baseline_row(
    name: &str,
    obvious: &[DelibScenario],
    sampled: &[DelibScenario],
    pick: impl Fn(&DelibScenario) -> usize,
) -> DelibRow {
    let a: Vec<usize> = obvious.iter().map(&pick).collect();
    let b: Vec<usize> = sampled.iter().map(&pick).collect();
    fill(name, obvious, sampled, &a, &b)
}

fn fill(
    name: &str,
    obvious: &[DelibScenario],
    sampled: &[DelibScenario],
    a: &[usize],
    b: &[usize],
) -> DelibRow {
    let accuracy_by_case = OBVIOUS_DELIBERATIONS
        .iter()
        .filter_map(|&case| {
            rate(
                obvious
                    .iter()
                    .zip(a)
                    .filter(|(s, _)| s.case == case)
                    .map(|(s, &c)| s.is_correct(c)),
            )
            .map(|r| (case, r))
        })
        .collect();
    let mut agreement_by_kind = [None; KINDS];
    for (k, slot) in agreement_by_kind.iter_mut().enumerate() {
        *slot = rate(
            sampled
                .iter()
                .zip(b)
                .filter(|(s, _)| s.topic() == k)
                .map(|(s, &c)| s.is_correct(c)),
        );
    }
    DelibRow {
        name: name.to_string(),
        accuracy: rate(obvious.iter().zip(a).map(|(s, &c)| s.is_correct(c))),
        accuracy_by_case,
        agreement: rate(sampled.iter().zip(b).map(|(s, &c)| s.is_correct(c))),
        agreement_by_kind,
        ..DelibRow::default()
    }
}

/// Riga per un modello: `probs_a`/`probs_b` sono le sue probabilità su (a)
/// e (b) (da [`predict_all`]), miscelate con la regola con peso `prior_weight`.
pub fn delib_model_row(
    name: &str,
    obvious: &[DelibScenario],
    sampled: &[DelibScenario],
    probs_a: &[Vec<f32>],
    probs_b: &[Vec<f32>],
    prior_weight: f32,
    throughput: Option<f64>,
) -> DelibRow {
    let decide = |scenarios: &[DelibScenario], probs: &[Vec<f32>]| -> Vec<(usize, f32)> {
        scenarios
            .iter()
            .zip(probs)
            .map(|(s, p)| argmax(&blend(p, &s.rule, prior_weight)).unwrap_or((0, 0.0)))
            .collect()
    };
    let a = decide(obvious, probs_a);
    let b = decide(sampled, probs_b);
    let ca: Vec<usize> = a.iter().map(|x| x.0).collect();
    let cb: Vec<usize> = b.iter().map(|x| x.0).collect();
    let mut row = fill(name, obvious, sampled, &ca, &cb);
    let all: Vec<f32> = a.iter().chain(&b).map(|x| x.1).collect();
    let mut hist = [0usize; 5];
    for &p in &all {
        hist[((p * 5.0) as usize).min(4)] += 1;
    }
    row.confidence_hist = Some(hist);
    row.mean_confidence = (!all.is_empty()).then(|| all.iter().sum::<f32>() / all.len() as f32);
    row.coverage = THRESHOLDS
        .iter()
        .map(|&t| {
            let covered = all.iter().filter(|&&p| p >= t).count() as f32 / all.len().max(1) as f32;
            let acc = rate(
                obvious
                    .iter()
                    .zip(&a)
                    .filter(|(_, x)| x.1 >= t)
                    .map(|(s, x)| s.is_correct(x.0)),
            );
            let agr = rate(
                sampled
                    .iter()
                    .zip(&b)
                    .filter(|(_, x)| x.1 >= t)
                    .map(|(s, x)| s.is_correct(x.0)),
            );
            (t, covered, acc, agr)
        })
        .collect();
    row.throughput = throughput;
    row
}

fn pct(v: Option<f32>) -> String {
    v.map_or("—".to_string(), |v| format!("{:.0}%", v * 100.0))
}

/// Tabella compatta: (a), (b), accordo per tipo, confidenza, righe/s.
pub fn format_delib_table(rows: &[DelibRow]) -> String {
    let mut out = format!(
        "{:<26} {:>7} {:>7} {:>6} {:>6} {:>6} {:>6} {:>6} {:>18} {:>7}\n",
        "cervello",
        "ovvi a",
        "acc. b",
        "coppia",
        "figlio",
        "furto",
        "prot.",
        "conf.",
        "conf. 0-.2-.4-.6-.8-1",
        "righe/s"
    );
    for r in rows {
        if let Some(e) = &r.error {
            out.push_str(&format!("{:<26} errore: {e}\n", r.name));
            continue;
        }
        let hist = r.confidence_hist.map_or("—".to_string(), |h| {
            h.iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("/")
        });
        out.push_str(&format!(
            "{:<26} {:>7} {:>7} {:>6} {:>6} {:>6} {:>6} {:>6} {:>18} {:>7}\n",
            r.name,
            pct(r.accuracy),
            pct(r.agreement),
            pct(r.agreement_by_kind[0]),
            pct(r.agreement_by_kind[1]),
            pct(r.agreement_by_kind[2]),
            pct(r.agreement_by_kind[3]),
            r.mean_confidence
                .map_or("—".to_string(), |c| format!("{c:.2}")),
            hist,
            r.throughput.map_or("—".to_string(), |t| format!("{t:.1}")),
        ));
    }
    out
}

/// Tabella delle soglie: per ogni modello, quante risposte passano e quanto sono giuste.
pub fn format_threshold_table(rows: &[DelibRow]) -> String {
    let mut out = format!("{:<26}", "soglia: coperte / a / b");
    for t in THRESHOLDS {
        out.push_str(&format!(" {:>18}", format!("≥ {t:.1}")));
    }
    out.push('\n');
    for r in rows.iter().filter(|r| !r.coverage.is_empty()) {
        out.push_str(&format!("{:<26}", r.name));
        for &(_, covered, a, b) in &r.coverage {
            let cell = format!("{:.0}% / {} / {}", covered * 100.0, pct(a), pct(b));
            out.push_str(&format!(" {cell:>18}"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockModel;

    #[test]
    fn obvious_deliberations_cover_every_case_and_rules_agree() {
        let scenarios = obvious_deliberations(3, 2);
        for case in OBVIOUS_DELIBERATIONS {
            assert!(
                scenarios.iter().any(|s| s.case == case),
                "nessuno scenario per {case}"
            );
        }
        for s in &scenarios {
            assert!(!s.acceptable.is_empty());
            assert_eq!(s.query.options.len(), s.deliberation.options.len());
            assert!((s.rule.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
        // I casi sono ovvi anche per la regola del `sim`.
        let rules = delib_baseline_row("regola", &scenarios, &[], DelibScenario::rule_argmax);
        assert!(rules.accuracy.unwrap() >= 0.9, "{rules:?}");
        let (probs, _) = predict_all(&mut MockModel::new(), &scenarios, 8).unwrap();
        let mock = delib_model_row("mock", &scenarios, &[], &probs, &[], 0.0, None);
        assert!(mock.accuracy.unwrap() >= 0.8, "{mock:?}");
    }

    #[test]
    fn sampled_deliberations_are_labeled_by_the_rule() {
        let scenarios = sampled_deliberations(5, 12);
        assert_eq!(scenarios.len(), 12);
        for s in &scenarios {
            assert_eq!(s.acceptable, vec![s.rule_argmax()]);
        }
        let row = delib_baseline_row("regola", &[], &scenarios, DelibScenario::rule_argmax);
        assert_eq!(row.agreement, Some(1.0));
        let table = format_delib_table(&[row]);
        assert!(table.contains("100%"), "{table}");
    }
}
