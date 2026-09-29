//! Deliberazioni: quando si aprono, testi, regole di ripiego ed effetti.
//!
//! Triggers (see [`crate::deliberation`] for the model):
//! - couple proposals: at midnight, instead of forming couples at once
//!   (see `World::propose_couples`);
//! - children: at midnight, when a couple's roll for a child passes and the
//!   administration would allow it (see `World::births`);
//! - theft: hourly while the Mercati are open ([`World::temptations`]);
//! - protests: when a birth is denied, and when the Mense run out of Razioni.
//!
//! Every tick ends with [`World::run_deliberations`]: new deliberations are
//! passed to the brain, its answers applied, and the rest resolved by the
//! built-in rule at the deadline (at once for brains that do not answer).

use rand::RngExt;
use serde::{Deserialize, Serialize};

use super::{World, price_at};
use crate::action::Action;
use crate::brain::Brain;
use crate::carriage::CarriageKind;
use crate::deliberation::{
    Choice, Deliberation, DeliberationAnswer, DeliberationId, DeliberationKind, DeliberationOption,
    Gathering, Grievance, ResolvedDeliberation, Resolver,
};
use crate::event::EventKind;
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::npc::{LifeStage, Npc, RelationKind, Sex};
use crate::time::{GameTime, MINUTES_PER_DAY, MINUTES_PER_HOUR};

/// Most rounds of "notify, poll, resolve" per tick (a resolution may open
/// new deliberations, e.g. a denied birth leads to a protest).
const MAX_ROUNDS: usize = 4;
/// Protest gatherings run between these hours (a later call waits for the next morning).
const PROTEST_FROM_HOUR: u32 = 8;
const PROTEST_UNTIL_HOUR: u32 = 18;

/// Something an NPC (or a pair) cannot do again until a given time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Cooldown {
    /// No new proposal between the two (smaller id first).
    Proposal(NpcId, NpcId),
    Theft(NpcId),
    Protest(NpcId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct CooldownUntil {
    what: Cooldown,
    until: GameTime,
}

fn pair(a: NpcId, b: NpcId) -> Cooldown {
    Cooldown::Proposal(a.min(b), a.max(b))
}

/// What `other` is to someone, as an Italian noun ("madre", "amico").
pub(super) fn relation_word(kind: RelationKind, other: Sex) -> &'static str {
    match kind {
        RelationKind::Partner => other.pick("compagna", "compagno"),
        RelationKind::Parent => other.pick("madre", "padre"),
        RelationKind::Child => other.pick("figlia", "figlio"),
        RelationKind::Sibling => other.pick("sorella", "fratello"),
        RelationKind::Friend => other.pick("amica", "amico"),
    }
}

fn tie_level(affinity: f32) -> &'static str {
    match affinity {
        a if a < 0.0 => "teso",
        a if a < 0.3 => "tiepido",
        a if a < 0.6 => "buono",
        a if a < 0.8 => "forte",
        _ => "fortissimo",
    }
}

/// Normalized weights (uniform if they are all 0).
fn normalize(mut weights: Vec<f32>) -> Vec<f32> {
    for w in &mut weights {
        if !w.is_finite() || *w < 0.0 {
            *w = 0.0;
        }
    }
    let sum: f32 = weights.iter().sum();
    let n = weights.len().max(1) as f32;
    for w in &mut weights {
        *w = if sum > 0.0 { *w / sum } else { 1.0 / n };
    }
    weights
}

type Composed = (String, String, Vec<DeliberationOption>);

fn option(choice: Choice, description: String) -> DeliberationOption {
    DeliberationOption {
        choice,
        description,
    }
}

impl World {
    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Deliberations waiting for a decision, oldest first.
    pub fn open_deliberations(&self) -> &[Deliberation] {
        &self.deliberations
    }

    /// An open deliberation.
    pub fn deliberation(&self, id: DeliberationId) -> Option<&Deliberation> {
        self.deliberation_position(id)
            .map(|pos| &self.deliberations[pos])
    }

    /// The open deliberation `npc` is deciding, if any (at most one at a time).
    pub fn deliberation_of(&self, npc: NpcId) -> Option<&Deliberation> {
        self.deliberations.iter().find(|d| d.npc == npc)
    }

    /// The last resolved deliberations, oldest first (at most
    /// [`crate::SimParams::recent_deliberations_kept`]).
    pub fn recent_deliberations(&self) -> &[ResolvedDeliberation] {
        &self.recent_deliberations
    }

    /// Protest gatherings called and not over yet.
    pub fn gatherings(&self) -> &[Gathering] {
        &self.gatherings
    }

    /// The protest `npc` is taking part in right now, if any.
    pub fn protest_of(&self, npc: NpcId) -> Option<&Gathering> {
        if self.gatherings.is_empty() {
            return None;
        }
        let now = self.clock;
        self.gatherings
            .iter()
            .find(|g| g.is_running(now) && g.members.contains(&npc))
    }

    /// Until when the administration allows more births after protests.
    pub fn birth_bonus_until(&self) -> Option<GameTime> {
        self.birth_bonus_until.filter(|&t| t > self.clock)
    }

    /// Probabilities the built-in rule would give each option of an open
    /// deliberation right now (same order as its options, summing to 1). The
    /// rule samples from them with the world's RNG.
    pub fn deliberation_rule_weights(&self, id: DeliberationId) -> Option<Vec<f32>> {
        self.deliberation(id).map(|d| self.rule_weights(d))
    }

    fn deliberation_position(&self, id: DeliberationId) -> Option<usize> {
        self.deliberations.binary_search_by_key(&id, |d| d.id).ok()
    }

    /// Whether `id` decides, or is the other half of, an open couple or child deliberation.
    pub(super) fn is_involved(&self, id: NpcId) -> bool {
        self.deliberations.iter().any(|d| {
            d.npc == id
                || matches!(
                    d.kind,
                    DeliberationKind::CoupleProposal { from: o } | DeliberationKind::HaveChild { partner: o }
                        if o == id
                )
        })
    }

    pub(super) fn on_cooldown(&self, what: Cooldown) -> bool {
        let now = self.clock;
        self.cooldowns
            .iter()
            .any(|c| c.what == what && c.until > now)
    }

    fn set_cooldown(&mut self, what: Cooldown, days: u64) {
        let until = self.clock + days * MINUTES_PER_DAY;
        match self.cooldowns.iter_mut().find(|c| c.what == what) {
            Some(c) => c.until = c.until.max(until),
            None => self.cooldowns.push(CooldownUntil { what, until }),
        }
    }

    pub(super) fn prune_cooldowns(&mut self) {
        let now = self.clock;
        self.cooldowns.retain(|c| c.until > now);
    }

    // ------------------------------------------------------------------
    // Opening
    // ------------------------------------------------------------------

    /// Opens a deliberation for `npc` (texts written now). None if the
    /// situation cannot be described (someone involved is gone, or the
    /// Mercato does not sell the item).
    ///
    /// The triggers call it with their own checks and cooldowns; it is public
    /// for tests, evaluations and scripted events, which may open any
    /// situation by hand (no check that it makes sense: e.g. a proposal
    /// between partners is cancelled only when it is resolved).
    pub fn open_deliberation(
        &mut self,
        npc: NpcId,
        kind: DeliberationKind,
    ) -> Option<DeliberationId> {
        let i = self.npc_index(npc)?;
        let (question, context, options) = self.compose(i, kind)?;
        let id = DeliberationId(self.next_deliberation_id);
        self.next_deliberation_id += 1;
        let asked = self.clock;
        let deadline = asked + self.params.deliberation_minutes(&kind).max(1);
        self.deliberations.push(Deliberation {
            id,
            npc,
            kind,
            asked,
            deadline,
            question,
            context,
            options,
        });
        self.deliberation_counters.opened[kind.index()] += 1;
        Some(id)
    }

    /// "Luca Bianchi (27 anni, operaio)".
    fn who(&self, n: &Npc) -> String {
        let what = match (n.job, n.stage()) {
            (Some(job), _) => job.name().to_string(),
            (None, LifeStage::Anziano) => n.sex.pick("pensionata", "pensionato").to_string(),
            (None, stage) => stage.name(n.sex).to_string(),
        };
        format!("{} ({} anni, {what})", n.name, n.age)
    }

    /// Question, context and options of a new deliberation for NPC `i`.
    fn compose(&self, i: usize, kind: DeliberationKind) -> Option<Composed> {
        let npc = &self.npcs[i];
        let mut context = self.npc_context(npc.id)?;
        context.push_str(&format!(
            " Di carattere è {}.",
            npc.traits.describe(npc.sex)
        ));
        let (question, situation, options) = match kind {
            DeliberationKind::CoupleProposal { from } => self.compose_couple(npc, from)?,
            DeliberationKind::HaveChild { partner } => self.compose_child(npc, partner)?,
            DeliberationKind::Theft {
                item,
                market,
                helper,
            } => self.compose_theft(npc, item, market, helper)?,
            DeliberationKind::Protest { grievance, place } => {
                self.compose_protest(npc, grievance, place)
            }
        };
        context.push('\n');
        context.push_str(&situation);
        Some((question, context, options))
    }

    fn compose_couple(&self, npc: &Npc, from: NpcId) -> Option<Composed> {
        let f = self.npc(from)?;
        let affinity = npc.affinity(from);
        let gap = npc.age.abs_diff(f.age);
        let mut s = format!(
            "{} {} ha chiesto di diventare una coppia. Il loro legame d'amicizia è {} (affinità {affinity:.2}); {}.",
            self.who(f),
            npc.sex.pick("le", "gli"),
            tie_level(affinity),
            match gap {
                0 => "hanno la stessa età".to_string(),
                1 => "la differenza d'età è di un anno".to_string(),
                n => format!("la differenza d'età è di {n} anni"),
            },
        );
        match f.children().count() {
            0 => {}
            1 => s.push_str(&format!(" {} ha già un figlio.", f.first_name())),
            n => s.push_str(&format!(" {} ha già {n} figli.", f.first_name())),
        }
        if f.home == npc.home {
            s.push_str(" Vivono già nello stesso dormitorio.");
        } else {
            s.push_str(" Se accetta, andranno a vivere insieme.");
        }
        s.push_str(
            " Se rifiuta, il loro legame ne soffrirà; può anche chiedere tempo per pensarci.",
        );
        let question = format!(
            "{} accetta la proposta di {} di diventare una coppia?",
            npc.name, f.name
        );
        let options = vec![
            option(
                Choice::Accept,
                format!(
                    "accetta e diventa {} di {}",
                    npc.sex.pick("la compagna", "il compagno"),
                    f.name
                ),
            ),
            option(Choice::Refuse, format!("rifiuta la proposta di {}", f.name)),
            option(
                Choice::AskForTime,
                format!("chiede a {} un po' di tempo per pensarci", f.name),
            ),
        ];
        Some((question, s, options))
    }

    fn compose_child(&self, npc: &Npc, partner: NpcId) -> Option<Composed> {
        let p = self.npc(partner)?;
        let population = self.npcs.len().max(1) as f32;
        let per_person = self.available(ItemKind::Razione) / population;
        let mut s = format!(
            "L'amministrazione oggi permetterebbe una nascita: ci sono cuccette libere e nelle Mense ci sono {per_person:.1} razioni a persona. "
        );
        let kids: Vec<String> = npc
            .children()
            .filter_map(|c| self.npc(c))
            .map(|c| format!("{} ({} anni)", c.first_name(), c.age))
            .collect();
        match kids.as_slice() {
            [] => s.push_str("Non ha ancora figli. "),
            [one] => s.push_str(&format!("Ha già un figlio: {one}. ")),
            _ => s.push_str(&format!(
                "Ha già {} figli: {}. ",
                kids.len(),
                kids.join(", ")
            )),
        }
        let affinity = npc.affinity(partner);
        let tokens = npc.inventory.tokens + p.inventory.tokens;
        let years_left = match self.params.fertile_max_age.saturating_sub(npc.age) {
            0 => "solo quest'anno".to_string(),
            1 => "ancora per un anno".to_string(),
            n => format!("ancora per {n} anni"),
        };
        s.push_str(&format!(
            "Il legame con {} è {} (affinità {affinity:.2}). Insieme hanno {tokens} gettoni. Potrà avere figli {years_left}. Un figlio è una bocca in più da sfamare e una cuccetta in meno sul treno.",
            self.who(p),
            tie_level(affinity),
        ));
        let question = format!(
            "{} vuole provare ad avere un figlio con {} adesso?",
            npc.name, p.name
        );
        let options = vec![
            option(
                Choice::TryForChild,
                format!("prova ad avere un figlio con {} adesso", p.name),
            ),
            option(
                Choice::Wait,
                "aspetta ancora prima di avere un figlio".to_string(),
            ),
        ];
        Some((question, s, options))
    }

    fn compose_theft(
        &self,
        npc: &Npc,
        item: ItemKind,
        market: CarriageId,
        helper: Option<NpcId>,
    ) -> Option<Composed> {
        let price = self.price(market, item)?;
        let label = self.carriage_label(market);
        let mut s = format!(
            "Al {label} c'è {} a {price} gettoni, ma {} ne ha solo {}. ",
            item.with_article(),
            npc.first_name(),
            npc.inventory.tokens
        );
        s.push_str(match item {
            ItemKind::Attrezzo => "Senza attrezzo lavora molto più lentamente. ",
            _ => "Senza un vestito caldo si stanca più in fretta. ",
        });
        match self.merchant_on_duty(market) {
            Some(m) => s.push_str(&format!("Al bancone c'è {}. ", m.name)),
            None => s.push_str("Al bancone non c'è nessuno. "),
        }
        let others = self.npcs_in(market).filter(|n| n.id != npc.id).count();
        s.push_str(&format!(
            "Nel mercato ci sono altre {others} persone. Chi viene sorpreso a rubare paga una multa e perde la fiducia di chi lo conosce."
        ));
        let helper = helper.and_then(|h| {
            let other = self.npc(h)?;
            let kind = npc.relation(h)?.kind;
            Some((other, relation_word(kind, other.sex)))
        });
        if let Some((h, word)) = helper {
            s.push_str(&format!(
                " Potrebbe chiedere aiuto a {} ({word}), che ha {} gettoni.",
                h.name, h.inventory.tokens
            ));
        }
        let question = format!(
            "{} non può permettersi {}: che cosa fa?",
            npc.name,
            item.with_article()
        );
        let mut options = vec![
            option(
                Choice::Steal,
                format!("ruba {} al {label}", item.with_article()),
            ),
            option(
                Choice::Save,
                if npc.job.is_some() {
                    "rinuncia per ora, lavora e mette da parte i gettoni".to_string()
                } else {
                    "rinuncia per ora e mette da parte i gettoni".to_string()
                },
            ),
        ];
        if let Some((h, word)) = helper {
            options.push(option(
                Choice::AskForHelp,
                format!("chiede qualche gettone a {} ({word})", h.name),
            ));
        }
        Some((question, s, options))
    }

    fn compose_protest(&self, npc: &Npc, grievance: Grievance, place: CarriageId) -> Composed {
        let label = self.carriage_label(place);
        let mut s = String::new();
        let question = match grievance {
            Grievance::BirthDenied => {
                let partner = npc
                    .partner()
                    .and_then(|p| self.npc(p))
                    .map_or("il partner".to_string(), |p| p.name.clone());
                let reason = self
                    .birth_permit()
                    .err()
                    .map_or("il treno non può permetterselo".to_string(), |r| {
                        r.to_string()
                    });
                s.push_str(&format!(
                    "L'amministrazione ha appena negato a {} e {partner} un figlio: {reason}. ",
                    npc.first_name()
                ));
                format!(
                    "{} protesta contro l'amministrazione, che {} ha negato un figlio?",
                    npc.name,
                    npc.sex.pick("le", "gli")
                )
            }
            Grievance::FoodShortage => {
                s.push_str(&format!(
                    "Le Mense sono rimaste senza razioni e {} ha fame (sazietà {:.2}). ",
                    npc.first_name(),
                    npc.needs.hunger
                ));
                format!(
                    "{} protesta contro l'amministrazione per la mancanza di razioni?",
                    npc.name
                )
            }
        };
        let now = self.clock;
        let window = self.params.protest_window_days * MINUTES_PER_DAY;
        let recent = self
            .protest_tally
            .iter()
            .filter(|(t, g)| *g == grievance && now.since(*t) < window)
            .count();
        let recent = match recent {
            0 => "nessuno ha protestato".to_string(),
            1 => "ha protestato una persona".to_string(),
            n => format!("hanno protestato {n} persone"),
        };
        s.push_str(&format!(
            "Di recente per lo stesso motivo {recent}; se saranno almeno {}, l'amministrazione potrebbe cedere. ",
            self.params.protest_threshold
        ));
        match self
            .gatherings
            .iter()
            .find(|g| g.grievance == grievance && g.end > now)
        {
            Some(g) => s.push_str(&format!(
                "Una protesta è già convocata in {} fino alle {:02}:{:02} ({} persone).",
                self.carriage_label(g.place),
                g.end.hour(),
                g.end.minute(),
                g.members.len()
            )),
            None => s.push_str(&format!(
                "Chi protesta si riunisce in {label} per {} ore.",
                self.params.protest_hours
            )),
        }
        if npc.job.is_some() {
            s.push_str(" Protestare vuol dire lasciare il lavoro per qualche ora.");
        }
        let mut options = vec![
            option(
                Choice::Protest,
                format!("si unisce alla protesta in {label}"),
            ),
            option(
                Choice::Endure,
                match grievance {
                    Grievance::BirthDenied => "accetta la decisione dell'amministrazione",
                    Grievance::FoodShortage => "sopporta e aspetta che tornino le razioni",
                }
                .to_string(),
            ),
        ];
        if npc.job.is_some() {
            options.push(option(
                Choice::WorkHarder,
                "si rassegna e lavora ancora di più".to_string(),
            ));
        }
        (question, s, options)
    }

    // ------------------------------------------------------------------
    // Triggers
    // ------------------------------------------------------------------

    /// Midnight: single adults propose to their dearest eligible friend above
    /// [`crate::SimParams::couple_affinity`] (unless a recent refusal or a
    /// request for time holds them back); the friend deliberates.
    pub(super) fn propose_couples(&mut self) {
        let threshold = self.params.couple_affinity;
        for i in 0..self.npcs.len() {
            let a = &self.npcs[i];
            if a.age < LifeStage::ADULTO_FROM || a.partner().is_some() || self.is_involved(a.id) {
                continue;
            }
            let best = a
                .relations_of(RelationKind::Friend)
                .filter(|r| r.affinity >= threshold)
                .filter_map(|r| {
                    let j = self.npc_index(r.other)?;
                    let b = &self.npcs[j];
                    (self.can_couple(a, b)
                        && !self.is_involved(b.id)
                        && !self.on_cooldown(pair(a.id, b.id)))
                    .then_some((r.affinity, b.id))
                })
                .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)));
            if let Some((_, to)) = best {
                let from = self.npcs[i].id;
                self.open_deliberation(to, DeliberationKind::CoupleProposal { from });
            }
        }
    }

    /// A birth for mother `i` and father `j` was denied: each of them may
    /// deliberate a protest.
    pub(super) fn protest_on_denial(&mut self, i: usize, j: usize) {
        let chance = self.params.protest_chance_on_denial * self.params.deliberation_rate;
        if chance <= 0.0 {
            return;
        }
        let place = self.protest_place();
        for id in [self.npcs[i].id, self.npcs[j].id] {
            if self.rng.random::<f32>() >= chance || !self.may_protest(id) {
                continue;
            }
            let days = self.params.protest_cooldown_days;
            self.set_cooldown(Cooldown::Protest(id), days);
            let grievance = Grievance::BirthDenied;
            self.open_deliberation(id, DeliberationKind::Protest { grievance, place });
        }
    }

    fn may_protest(&self, id: NpcId) -> bool {
        self.npc(id)
            .is_some_and(|n| n.age >= LifeStage::ADULTO_FROM)
            && !self.is_involved(id)
            && !self.on_cooldown(Cooldown::Protest(id))
    }

    /// The Mense ran out of Razioni: hungry adults may deliberate a protest.
    pub(super) fn food_protests(&mut self) {
        let chance = self.params.protest_chance_on_shortage * self.params.deliberation_rate;
        let max = self.params.protest_max_on_shortage as usize;
        if chance <= 0.0 || max == 0 {
            return;
        }
        let place = self.protest_place();
        let mut opened = 0;
        for i in 0..self.npcs.len() {
            if opened >= max {
                break;
            }
            let npc = &self.npcs[i];
            let id = npc.id;
            if npc.needs.hunger >= 0.6 || !self.may_protest(id) {
                continue;
            }
            if self.rng.random::<f32>() >= chance {
                continue;
            }
            let days = self.params.protest_cooldown_days;
            self.set_cooldown(Cooldown::Protest(id), days);
            let grievance = Grievance::FoodShortage;
            if self
                .open_deliberation(id, DeliberationKind::Protest { grievance, place })
                .is_some()
            {
                opened += 1;
            }
        }
    }

    /// Where protests gather: the front-most Mensa (else the head carriage).
    fn protest_place(&self) -> CarriageId {
        self.carriages
            .iter()
            .find(|c| c.kind == CarriageKind::Mensa)
            .map_or(CarriageId(0), |c| c.id)
    }

    /// Hourly, while the Mercati are open: NPCs (14+) who need an Attrezzo or
    /// Vestito they cannot afford, near a Mercato that has one, may be
    /// tempted to steal it.
    pub(super) fn temptations(&mut self) {
        let p = &self.params;
        let chance = p.theft_temptation_per_hour * p.deliberation_rate;
        if chance <= 0.0 || p.is_night(self.clock.hour()) {
            return;
        }
        let max_distance = p.theft_max_distance;
        let markets: Vec<CarriageId> = self
            .carriages
            .iter()
            .filter(|c| c.kind == CarriageKind::Mercato)
            .map(|c| c.id)
            .collect();
        if markets.is_empty() {
            return;
        }
        for i in 0..self.npcs.len() {
            let npc = &self.npcs[i];
            if npc.age < LifeStage::GIOVANE_FROM
                || !npc.is_awake()
                || matches!(npc.action, Action::Travel { .. })
            {
                continue;
            }
            let wanted = ItemKind::SOLD
                .into_iter()
                .filter(|&item| npc.wants(item))
                .find_map(|item| {
                    markets
                        .iter()
                        .filter(|m| m.distance(npc.carriage) <= max_distance)
                        .filter(|m| {
                            let c = &self.carriages[m.index()];
                            c.stock.count(item) >= 1
                                && price_at(&self.params, self.economy.pay_level, c, item)
                                    .is_some_and(|price| price > npc.inventory.tokens)
                        })
                        .min_by_key(|m| (m.distance(npc.carriage), m.0))
                        .map(|&m| (item, m))
                });
            let Some((item, market)) = wanted else {
                continue;
            };
            let id = npc.id;
            if self.is_involved(id) || self.on_cooldown(Cooldown::Theft(id)) {
                continue;
            }
            if self.rng.random::<f32>() >= chance {
                continue;
            }
            let helper = self.best_helper(i);
            let days = self.params.theft_cooldown_days;
            self.set_cooldown(Cooldown::Theft(id), days);
            self.open_deliberation(
                id,
                DeliberationKind::Theft {
                    item,
                    market,
                    helper,
                },
            );
        }
    }

    /// Who NPC `i` could ask for tokens: family or a friend with positive
    /// affinity who has some (closest first).
    fn best_helper(&self, i: usize) -> Option<NpcId> {
        let npc = &self.npcs[i];
        npc.relations
            .iter()
            .filter(|r| r.affinity > 0.2 || (r.kind.is_family() && r.affinity > 0.0))
            .filter_map(|r| {
                let other = self.npc(r.other)?;
                let family = if r.kind.is_family() { 0.3 } else { 0.0 };
                (other.inventory.tokens >= 2).then_some((r.affinity + family, r.other))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|(_, id)| id)
    }

    // ------------------------------------------------------------------
    // Resolution
    // ------------------------------------------------------------------

    /// End of every tick: new deliberations go to the brain, its answers are
    /// applied, and the built-in rule resolves the others at their deadline
    /// (immediately if the brain does not answer deliberations).
    pub(super) fn run_deliberations(&mut self, brain: &mut dyn Brain) {
        let answers_async = brain.answers_deliberations();
        for _ in 0..MAX_ROUNDS {
            let start = self
                .deliberations
                .partition_point(|d| d.id.0 < self.notified_until);
            if start < self.deliberations.len() {
                self.notified_until = self.next_deliberation_id;
                if answers_async {
                    let asked: Vec<EventKind> = self.deliberations[start..]
                        .iter()
                        .map(|d| EventKind::DeliberationAsked {
                            id: d.id,
                            npc: d.npc,
                            name: self.npc(d.npc).map(|n| n.name.clone()).unwrap_or_default(),
                            kind: d.kind,
                            question: d.question.clone(),
                        })
                        .collect();
                    for kind in asked {
                        self.push_event(kind);
                    }
                }
                brain.deliberations_opened(self, &self.deliberations[start..]);
            }
            for answer in brain.deliberations_resolved(self) {
                self.apply_answer(answer);
            }
            let now = self.clock;
            let limit = self.next_deliberation_id;
            while let Some(pos) = self
                .deliberations
                .iter()
                .position(|d| d.id.0 < limit && (!answers_async || d.deadline <= now))
            {
                let choice = self.rule_choice(pos);
                self.resolve(pos, choice, Resolver::Rules, None);
            }
            if self
                .deliberations
                .last()
                .is_none_or(|d| d.id.0 < self.notified_until)
            {
                break;
            }
        }
    }

    fn apply_answer(&mut self, answer: DeliberationAnswer) {
        match self.deliberation_position(answer.id) {
            Some(pos) if answer.choice < self.deliberations[pos].options.len() => {
                let confidence = if answer.confidence.is_finite() {
                    answer.confidence.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                self.resolve(pos, answer.choice, Resolver::Brain, Some(confidence));
            }
            _ => self.deliberation_counters.answers_ignored += 1,
        }
    }

    /// The rule's pick for the deliberation at `pos` (consumes world randomness).
    fn rule_choice(&mut self, pos: usize) -> usize {
        let weights = self.rule_weights(&self.deliberations[pos]);
        let roll = self.rng.random::<f32>();
        let mut acc = 0.0;
        for (k, w) in weights.iter().enumerate() {
            acc += w;
            if roll < acc {
                return k;
            }
        }
        weights.len().saturating_sub(1)
    }

    /// The built-in rule: a weight per option from the NPC's situation and
    /// character, normalized.
    fn rule_weights(&self, d: &Deliberation) -> Vec<f32> {
        let Some(npc) = self.npc(d.npc) else {
            return normalize(vec![1.0; d.options.len()]);
        };
        let t = npc.traits;
        let p = &self.params;
        let raw: Vec<f32> = match d.kind {
            DeliberationKind::CoupleProposal { from } => {
                let other = self.npc(from);
                let threshold = p.couple_affinity.min(0.99);
                let a = ((npc.affinity(from) - threshold) / (1.0 - threshold)).clamp(0.0, 1.0);
                let gap = other.map_or(0, |o| o.age.abs_diff(npc.age)) as f32;
                let excess = (gap - 4.0).max(0.0);
                let kids = |n: &Npc| n.children().next().is_some();
                let mut accept = 0.45 + 0.5 * a - 0.04 * excess + 0.1 * (t.boldness - 0.5);
                if kids(npc) {
                    accept -= 0.1;
                }
                if other.is_some_and(kids) {
                    accept -= 0.1;
                }
                let accept = accept.clamp(0.05, 0.95);
                let refuse = 0.08 + 0.2 * (1.0 - a) + 0.03 * excess;
                let time = 0.12 + 0.2 * (1.0 - a);
                d.options
                    .iter()
                    .map(|o| match o.choice {
                        Choice::Accept => accept,
                        Choice::Refuse => refuse,
                        _ => time,
                    })
                    .collect()
            }
            DeliberationKind::HaveChild { partner } => {
                let kids = npc.children().count();
                let mut w = 0.8;
                if kids == 0 {
                    w += 0.1;
                } else {
                    w -= 0.12 * (kids - 1) as f32;
                }
                if npc.age > 35 {
                    w -= 0.03 * (npc.age - 35) as f32;
                }
                if npc.age < 21 {
                    w -= 0.15;
                }
                let min = p.birth_min_razioni_per_person.max(0.1);
                let per_person = self.available(ItemKind::Razione) / self.npcs.len().max(1) as f32;
                let food = ((per_person - min) / (2.0 * min)).clamp(0.0, 1.0);
                w += 0.2 * (food - 0.5);
                let tokens =
                    npc.inventory.tokens + self.npc(partner).map_or(0, |n| n.inventory.tokens);
                if tokens < 20 {
                    w -= 0.1;
                }
                w += 0.2 * (npc.affinity(partner) - 0.6);
                let w = w.clamp(0.05, 0.95);
                d.options
                    .iter()
                    .map(|o| match o.choice {
                        Choice::TryForChild => w,
                        _ => 1.0 - w,
                    })
                    .collect()
            }
            DeliberationKind::Theft {
                item,
                market,
                helper,
            } => {
                let price = self.price(market, item).unwrap_or(item.base_value()).max(1) as f32;
                let short = (1.0 - npc.inventory.tokens as f32 / price).clamp(0.0, 1.0);
                let need = match item {
                    ItemKind::Attrezzo => 1.0,
                    _ if npc.needs.energy < 0.4 => 1.0,
                    _ => 0.7,
                };
                let desperation = short * need;
                let steal = 0.05 + 0.9 * desperation * (1.0 - t.honesty) * (0.5 + t.boldness);
                let save = 0.2 + 0.6 * t.honesty;
                let help = helper.and_then(|h| npc.relation(h)).map_or(0.0, |r| {
                    let family = if r.kind.is_family() { 0.15 } else { 0.0 };
                    0.1 + 0.5 * r.affinity.max(0.0) + family
                });
                d.options
                    .iter()
                    .map(|o| match o.choice {
                        Choice::Steal => steal,
                        Choice::Save => save,
                        _ => help,
                    })
                    .collect()
            }
            DeliberationKind::Protest { grievance, .. } => {
                let severity = match grievance {
                    Grievance::BirthDenied if npc.children().next().is_none() => 0.3,
                    Grievance::BirthDenied => 0.1,
                    Grievance::FoodShortage => 0.4 * (1.0 - npc.needs.hunger),
                };
                let protest = 0.05 + 0.5 * t.boldness + severity;
                let endure = 0.15 + 0.45 * (1.0 - t.boldness);
                d.options
                    .iter()
                    .map(|o| match o.choice {
                        Choice::Protest => protest,
                        Choice::Endure => endure,
                        _ => 0.25,
                    })
                    .collect()
            }
        };
        normalize(raw)
    }

    /// Whether the premises of a deliberation still hold (everyone alive,
    /// still single for a proposal, still partners for a child, still
    /// needing the item for a theft).
    fn still_relevant(&self, d: &Deliberation) -> bool {
        let Some(npc) = self.npc(d.npc) else {
            return false;
        };
        match d.kind {
            DeliberationKind::CoupleProposal { from } => {
                self.npc(from).is_some_and(|f| self.can_couple(npc, f))
            }
            DeliberationKind::HaveChild { partner } => {
                npc.partner() == Some(partner) && self.npc(partner).is_some()
            }
            // Bought (or was given) the item meanwhile: nothing to decide.
            DeliberationKind::Theft { item, .. } => npc.wants(item),
            DeliberationKind::Protest { .. } => true,
        }
    }

    /// Closes the deliberation at `pos` with option `choice`: logs it and
    /// applies its effects (or cancels it if its premises no longer hold).
    fn resolve(&mut self, pos: usize, choice: usize, by: Resolver, confidence: Option<f32>) {
        let d = self.deliberations.remove(pos);
        if !self.still_relevant(&d) {
            self.deliberation_counters.cancelled += 1;
            return;
        }
        let Some(chosen) = d.options.get(choice).cloned() else {
            self.deliberation_counters.cancelled += 1;
            return;
        };
        let k = d.kind.index();
        let counters = &mut self.deliberation_counters;
        match by {
            Resolver::Rules => counters.by_rules[k] += 1,
            Resolver::Brain => counters.by_brain[k] += 1,
        }
        counters.choices[chosen.choice.index()] += 1;
        let name = self.npc(d.npc).map(|n| n.name.clone()).unwrap_or_default();
        self.push_event(EventKind::DeliberationResolved {
            id: d.id,
            npc: d.npc,
            name,
            kind: d.kind,
            choice: chosen.choice,
            description: chosen.description.clone(),
            by,
            confidence,
        });
        self.apply_choice(&d, chosen.choice);
        self.recent_deliberations.push(ResolvedDeliberation {
            deliberation: d,
            choice,
            by,
            confidence,
            resolved: self.clock,
        });
        let keep = self.params.recent_deliberations_kept;
        if self.recent_deliberations.len() > keep {
            let extra = self.recent_deliberations.len() - keep;
            self.recent_deliberations.drain(..extra);
        }
    }

    fn apply_choice(&mut self, d: &Deliberation, choice: Choice) {
        let Some(i) = self.npc_index(d.npc) else {
            return;
        };
        match (d.kind, choice) {
            (DeliberationKind::CoupleProposal { from }, _) => {
                let Some(j) = self.npc_index(from) else {
                    return;
                };
                match choice {
                    Choice::Accept => {
                        let mut residents = self.resident_counts();
                        self.couple(j, i, &mut residents);
                    }
                    Choice::Refuse => {
                        let drop = self.params.proposal_refused_affinity;
                        self.add_affinity(i, j, -drop);
                        let days = self.params.proposal_refused_days;
                        self.set_cooldown(pair(d.npc, from), days);
                    }
                    _ => {
                        let days = self.params.proposal_retry_days;
                        self.set_cooldown(pair(d.npc, from), days);
                    }
                }
            }
            (DeliberationKind::HaveChild { partner }, Choice::TryForChild) => {
                if let Some(j) = self.npc_index(partner) {
                    let mut residents = self.resident_counts();
                    self.attempt_birth(i, j, &mut residents);
                }
            }
            (DeliberationKind::Theft { item, market, .. }, Choice::Steal) => {
                self.steal(i, item, market);
            }
            (
                DeliberationKind::Theft {
                    item,
                    market,
                    helper: Some(helper),
                },
                Choice::AskForHelp,
            ) => self.ask_for_help(i, helper, item, market),
            (DeliberationKind::Protest { grievance, place }, Choice::Protest) => {
                self.join_protest(i, grievance, place);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Effects
    // ------------------------------------------------------------------

    /// NPC `i` tries to steal one `item` at `market`. Caught: the item stays,
    /// a fine is paid and everyone who knows them trusts them less.
    fn steal(&mut self, i: usize, item: ItemKind, market: CarriageId) {
        let Some(c) = self.carriages.get(market.index()) else {
            return;
        };
        if c.stock.count(item) < 1 {
            return;
        }
        let level = self.economy.pay_level;
        let price = price_at(&self.params, level, c, item).unwrap_or(item.base_value());
        let thief = self.npcs[i].id;
        let guard = self.merchant_on_duty(market).is_some();
        let witnesses = self
            .npcs_in(market)
            .filter(|n| n.id != thief && n.is_awake())
            .count() as f32;
        let p = &self.params;
        let mut chance = p.theft_caught_base + (p.theft_caught_per_witness * witnesses).min(0.3)
            - 0.2 * (self.npcs[i].traits.boldness - 0.5);
        if guard {
            chance += p.theft_caught_merchant;
        }
        let caught = self.rng.random::<f32>() < chance.clamp(0.0, 1.0);
        self.deliberation_counters.thefts += 1;
        let mut fine = 0;
        if caught {
            self.deliberation_counters.thefts_caught += 1;
            let tokens = &mut self.npcs[i].inventory.tokens;
            fine = price.min(*tokens);
            *tokens -= fine;
            self.economy.treasury += u64::from(fine);
            self.economy.counters.fines += u64::from(fine);
            let drop = self.params.theft_caught_affinity;
            let known: Vec<usize> = self.npcs[i]
                .relations
                .iter()
                .filter_map(|r| self.npc_index(r.other))
                .collect();
            for j in known {
                self.add_affinity(i, j, -drop);
            }
        } else {
            self.carriages[market.index()].stock.take(item, 1.0);
            if let Some(slot) = self.npcs[i].inventory.slot_mut(item) {
                *slot = Some(1.0);
            }
        }
        let npc = &self.npcs[i];
        let kind = EventKind::Theft {
            npc: npc.id,
            name: npc.name.clone(),
            sex: npc.sex,
            item,
            carriage: market,
            caught,
            fine,
        };
        self.push_event(kind);
    }

    /// NPC `i` asks `helper` for the tokens it lacks; the closer they are,
    /// the likelier the help (at most half of the helper's tokens).
    fn ask_for_help(&mut self, i: usize, helper: NpcId, item: ItemKind, market: CarriageId) {
        let Some(h) = self.npc_index(helper) else {
            return;
        };
        let asker = self.npcs[i].id;
        let price = self.price(market, item).unwrap_or(item.base_value());
        let shortfall = price.saturating_sub(self.npcs[i].inventory.tokens).max(1);
        let (affinity, family) = self.npcs[h]
            .relation(asker)
            .map_or((0.0, false), |r| (r.affinity, r.kind.is_family()));
        let chance = (0.3 + 0.5 * affinity + if family { 0.2 } else { 0.0 }).clamp(0.0, 1.0);
        let gift = if self.rng.random::<f32>() < chance {
            shortfall.min(self.npcs[h].inventory.tokens / 2)
        } else {
            0
        };
        if gift > 0 {
            self.npcs[h].inventory.tokens -= gift;
            self.npcs[i].inventory.tokens += gift;
            self.economy.counters.help += u64::from(gift);
            self.add_affinity(i, h, self.params.affinity_per_chat);
            self.deliberation_counters.help_given += 1;
        } else {
            self.add_affinity(i, h, -self.params.affinity_per_chat);
            self.deliberation_counters.help_refused += 1;
        }
        let kind = EventKind::HelpAsked {
            npc: asker,
            name: self.npcs[i].name.clone(),
            helper,
            helper_name: self.npcs[h].name.clone(),
            tokens: gift,
        };
        self.push_event(kind);
    }

    /// NPC `i` joins the protest about `grievance` (calling one in `place` if
    /// none is planned); the administration may give in.
    fn join_protest(&mut self, i: usize, grievance: Grievance, place: CarriageId) {
        let id = self.npcs[i].id;
        let now = self.clock;
        self.protest_tally.push((now, grievance));
        match self
            .gatherings
            .iter_mut()
            .find(|g| g.grievance == grievance && g.end > now)
        {
            Some(g) => {
                if !g.members.contains(&id) {
                    g.members.push(id);
                }
            }
            None => {
                let start = if (PROTEST_FROM_HOUR..PROTEST_UNTIL_HOUR).contains(&now.hour()) {
                    now
                } else {
                    now.next_at(PROTEST_FROM_HOUR, 0)
                };
                let end = start + self.params.protest_hours.max(1) * MINUTES_PER_HOUR;
                self.gatherings.push(Gathering {
                    grievance,
                    place,
                    start,
                    end,
                    members: vec![id],
                });
                self.deliberation_counters.protests_called += 1;
                self.push_event(EventKind::ProtestCalled {
                    grievance,
                    place,
                    start,
                    end,
                });
            }
        }
        self.admin_response(grievance);
    }

    /// Enough recent protesters about `grievance`: the administration gives
    /// in (more births for a while, or emergency rations for the hungry).
    fn admin_response(&mut self, grievance: Grievance) {
        let now = self.clock;
        let window = self.params.protest_window_days * MINUTES_PER_DAY;
        self.protest_tally.retain(|(t, _)| now.since(*t) < window);
        let protesters = self
            .protest_tally
            .iter()
            .filter(|(_, g)| *g == grievance)
            .count() as u32;
        if protesters < self.params.protest_threshold.max(1) {
            return;
        }
        let until = match grievance {
            Grievance::BirthDenied => {
                if self.birth_bonus_until().is_some() {
                    return;
                }
                let until = now + self.params.protest_concession_days * MINUTES_PER_DAY;
                self.birth_bonus_until = Some(until);
                Some(until)
            }
            Grievance::FoodShortage => {
                let restore = self.params.meal_restore;
                for npc in self.npcs.iter_mut().filter(|n| n.needs.hunger < 0.5) {
                    npc.needs.hunger = (npc.needs.hunger + restore).min(1.0);
                    npc.starving_minutes = 0;
                }
                None
            }
        };
        self.protest_tally.retain(|(_, g)| *g != grievance);
        self.deliberation_counters.concessions += 1;
        self.push_event(EventKind::AdminConceded {
            grievance,
            protesters,
            until,
        });
    }

    /// Hourly: gatherings that are over are forgotten.
    pub(super) fn end_gatherings(&mut self) {
        let now = self.clock;
        self.gatherings.retain(|g| g.end > now);
    }

    /// Where NPC `id` must be for a running protest, and until when.
    pub(super) fn protest_duty(&self, id: NpcId) -> Option<(CarriageId, GameTime)> {
        self.protest_of(id).map(|g| (g.place, g.end))
    }

    /// `id` died: its deliberations (and those it was the other half of) are
    /// cancelled, and it leaves any protest.
    pub(super) fn forget_deliberations_of(&mut self, id: NpcId) {
        let before = self.deliberations.len();
        self.deliberations.retain(|d| {
            d.npc != id
                && !matches!(
                    d.kind,
                    DeliberationKind::CoupleProposal { from: o } | DeliberationKind::HaveChild { partner: o }
                        if o == id
                )
        });
        self.deliberation_counters.cancelled += (before - self.deliberations.len()) as u64;
        for g in &mut self.gatherings {
            g.members.retain(|&m| m != id);
        }
    }
}
