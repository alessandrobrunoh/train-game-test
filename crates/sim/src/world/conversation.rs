//! Conversazioni tra NPC (vedi [`crate::dialogue`]).
//!
//! When NPC A starts `Socialize(B)` ([`World::start_chat`]):
//! - B free in the same place ([`World::same_place`]), awake, idling or in
//!   a one-sided chat, not in another conversation nor at a protest, and
//!   not too hostile to A (`conversation_refuse_affinity`): B switches to
//!   `Socialize(A)` with the same end and a [`Conversation`] opens. Both
//!   gain `socialize_gain` per minute while talking.
//! - B eating, with at least `conversation_min_minutes` of meal left: B
//!   keeps eating and talks along; the conversation (and A's chat) ends
//!   with the meal at the latest, and B gets `socialize_partner_bonus`.
//! - Otherwise (working, asleep, travelling, queuing, busy talking): a
//!   one-sided chat, a quick word of at most `one_sided_chat_minutes`, with
//!   no conversation and the old end-of-chat effects (see `finish_action`).
//!
//! A conversation gets a [`Topic`] (from the pair's tie, needs, jobs, recent
//! notable events they may know of, grievances; weighted, seeded), a
//! [`Tone`] (affinity, personality compatibility, grumpy/kind/cheerful
//! tags, `quarrel_chance`) and 3–6 lines written at once
//! ([`crate::dialogue::text`]). It ends for both at `until`
//! ([`World::end_conversations`], at the start of the tick: affinity by
//! tone and compatibility, gossip changes the listener's opinion of whom it
//! was about), or early when one of them dies. Quarrels and gossip about a
//! theft are logged as [`EventKind::Chat`], rate-limited.

use rand::RngExt;
use serde::{Deserialize, Serialize};

use super::deliberate::relation_word;
use super::{World, index_of};
use crate::action::Action;
use crate::dialogue::text::{self, Need, Script, Subject, Voice};
use crate::dialogue::{Conversation, ConversationId, News, Tone, Topic};
use crate::event::EventKind;
use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::npc::{LifeStage, Npc, RelationKind, Sex};
use crate::personality::Temper;
use crate::time::{GameTime, MINUTES_PER_DAY, MINUTES_PER_HOUR};

/// Notable facts kept for conversations (the newest).
const NEWS_KEPT: usize = 48;
/// Under this age people don't gossip, complain nor talk about work.
const KID_AGE: u32 = 14;
/// Under this age people only babble (family or small talk).
const BABY_AGE: u32 = 4;
/// Generic gossip: only about people liked or disliked at least this much.
const GOSSIP_MIN_AFFINITY: f32 = 0.3;

/// How a partner reacts to someone who wants to chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Join {
    /// Stops idling and talks: a two-sided conversation.
    Switch,
    /// Keeps eating and talks along.
    WhileEating,
    /// Busy: a one-sided chat.
    Busy,
}

/// A notable fact people can talk about, taken from an event as it is
/// logged ([`World::collect_news`]). It holds only what the event says (the
/// rest is looked up when talking), so capping the event log never changes
/// the simulation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct NewsItem {
    time: GameTime,
    news: News,
    /// Whom it is about (the conversation's `about`), if anyone.
    about: Option<NpcId>,
    /// Everyone involved: they know it firsthand, their friends care more.
    involved: [Option<NpcId>; 3],
    /// Where it happened, if the event says so...
    place: Option<CarriageId>,
    /// ...else whose home it concerns.
    home_of: Option<NpcId>,
    /// Full name of whom it is about ("" if nobody), their sex if the event
    /// says it, and their partner's name (couples).
    name: String,
    sex: Option<Sex>,
    other: String,
}

impl NewsItem {
    fn involves(&self, id: NpcId) -> bool {
        self.involved.contains(&Some(id))
    }
}

/// What a conversation will be about.
struct Plan {
    topic: Topic,
    news: Option<News>,
    about: Option<NpcId>,
    subject: Option<Subject>,
    fond: bool,
    need: Need,
}

/// "Marta Rossi" → ("Marta", "Rossi").
fn split_name(name: &str) -> (&str, &str) {
    name.split_once(' ').unwrap_or((name, name))
}

fn subject(name: &str, sex: Sex, other: &str) -> Subject {
    let (first, surname) = split_name(name);
    Subject {
        first: first.to_string(),
        surname: surname.to_string(),
        sex,
        other: split_name(other).0.to_string(),
    }
}

pub(super) fn voice(n: &Npc) -> Voice<'_> {
    Voice {
        id: n.id,
        first: n.first_name(),
        sex: n.sex,
        age: n.age,
        job: n.job,
        personality: n.personality(),
    }
}

/// Picks an index with probability proportional to `weights` (None if all 0).
fn weighted(weights: &[f32], rng: &mut impl rand::Rng) -> Option<usize> {
    let total: f32 = weights.iter().filter(|w| w.is_finite() && **w > 0.0).sum();
    if total <= 0.0 {
        return None;
    }
    let mut r = rng.random::<f32>() * total;
    let mut last = None;
    for (k, &w) in weights.iter().enumerate() {
        if !(w.is_finite() && w > 0.0) {
            continue;
        }
        last = Some(k);
        if r < w {
            return Some(k);
        }
        r -= w;
    }
    last
}

impl World {
    /// Conversazioni in corso, ordinate per id.
    pub fn conversations(&self) -> &[Conversation] {
        &self.conversations
    }

    /// La conversazione in corso a cui partecipa `npc`, se c'è.
    pub fn conversation_of(&self, npc: NpcId) -> Option<&Conversation> {
        self.conversations.iter().find(|c| c.involves(npc))
    }

    /// Ultime conversazioni concluse, dalla più vecchia (al massimo
    /// `SimParams::recent_conversations_kept`).
    pub fn recent_conversations(&self) -> &[Conversation] {
        &self.recent_conversations
    }

    /// The topic a chat between `a` and `b` would most likely have (without
    /// randomness nor news: for option descriptions).
    pub fn likely_topic(&self, a: NpcId, b: NpcId) -> Option<Topic> {
        let (a, b) = (self.npc(a)?, self.npc(b)?);
        let grievance = if self.shortages[ItemKind::Razione.index()] {
            1.0
        } else {
            0.0
        };
        let gossip = a.personality().has(Temper::Pettegolo)
            && a.relations
                .iter()
                .any(|r| r.other != b.id && r.affinity.abs() >= GOSSIP_MIN_AFFINITY);
        let w = self.topic_weights(a, b, false, gossip, grievance);
        Topic::ALL
            .into_iter()
            .zip(w)
            .filter(|&(_, w)| w > 0.0)
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(t, _)| t)
    }

    /// "chiacchiera con Marta Rossi (amica) di lavoro (30 min)".
    pub(super) fn describe_chat(&self, npc: &Npc, other: NpcId, minutes: u64) -> String {
        let Some(b) = self.npc(other) else {
            return format!("chiacchiera ({minutes} min)");
        };
        let tie = match npc.relation(other) {
            Some(r) if r.kind.is_family() => Some(relation_word(r.kind, b.sex)),
            Some(r) if r.affinity >= GOSSIP_MIN_AFFINITY => Some(b.sex.pick("amica", "amico")),
            Some(r) if r.affinity < 0.0 => Some(b.sex.pick("antipatica", "antipatico")),
            Some(_) => Some("conoscente"),
            None => None,
        };
        let topic = self
            .likely_topic(npc.id, other)
            .map_or("", Topic::about_phrase);
        match tie {
            Some(tie) => format!("chiacchiera con {} ({tie}) {topic} ({minutes} min)", b.name),
            None => format!("chiacchiera con {} {topic} ({minutes} min)", b.name),
        }
    }

    // ------------------------------------------------------------------
    // Start
    // ------------------------------------------------------------------

    /// NPC `i` starts chatting with `partner` for `minutes`: opens a
    /// conversation (switching the partner to `Socialize(i)`) or, if the
    /// partner is busy, a short one-sided chat. Returns the chat's length.
    pub(super) fn start_chat(&mut self, i: usize, partner: NpcId, minutes: u64) -> u64 {
        let now = self.clock;
        self.conversation_counters.chats += 1;
        let j = self.npc_index(partner);
        let join = j.map_or(Join::Busy, |j| self.join_mode(i, j));
        match (join, j) {
            (Join::Switch, Some(j)) => {
                let until = now + minutes;
                let a = self.npcs[i].id;
                let b = &mut self.npcs[j];
                b.action = Action::Socialize(a);
                b.action_since = now;
                b.action_until = until;
                self.open_conversation(i, j, until);
                minutes
            }
            (Join::WhileEating, Some(j)) => {
                let until = self.npcs[j].action_until.min(now + minutes);
                self.conversation_counters.while_eating += 1;
                self.open_conversation(i, j, until);
                until.since(now)
            }
            _ => {
                self.conversation_counters.one_sided += 1;
                minutes.min(self.params.one_sided_chat_minutes.max(1))
            }
        }
    }

    /// How NPC `j` reacts to NPC `i` wanting to chat.
    fn join_mode(&self, i: usize, j: usize) -> Join {
        let (a, b) = (&self.npcs[i], &self.npcs[j]);
        if i == j
            || !self.same_place(a, b)
            || self.conversation_of(b.id).is_some()
            || self.protest_of(b.id).is_some()
            || b.affinity(a.id) < self.params.conversation_refuse_affinity
        {
            return Join::Busy;
        }
        match b.action {
            Action::Idle | Action::Socialize(_) => Join::Switch,
            Action::Eat(_)
                if b.action_until.since(self.clock) >= self.params.conversation_min_minutes =>
            {
                Join::WhileEating
            }
            _ => Join::Busy,
        }
    }

    /// Opens a conversation between NPCs `i` (who starts) and `j`, until `until`.
    fn open_conversation(&mut self, i: usize, j: usize, until: GameTime) {
        let now = self.clock;
        let plan = self.plan(i, j);
        let tone = self.choose_tone(i, j, plan.topic);
        let seed = self.rng.random::<u64>();
        let (a, b) = (&self.npcs[i], &self.npcs[j]);
        let place = &self.carriages[a.carriage.index()].name;
        let script = Script {
            topic: plan.topic,
            tone,
            news: plan.news,
            a: voice(a),
            b: voice(b),
            tie: a.relation(b.id).map(|r| r.kind).filter(|k| k.is_family()),
            about: plan.subject.as_ref(),
            fond: plan.fond,
            need: plan.need,
            place,
            hour: now.hour(),
        };
        let lines = text::write(&script, seed, now, until);
        let conversation = Conversation {
            id: ConversationId(self.next_conversation_id),
            a: a.id,
            b: b.id,
            carriage: a.carriage,
            since: now,
            until,
            topic: plan.topic,
            about: plan.about,
            tone,
            lines,
            news: plan.news,
        };
        self.next_conversation_id += 1;
        let c = &mut self.conversation_counters;
        c.conversations += 1;
        c.by_topic[plan.topic.index()] += 1;
        c.by_tone[tone.index()] += 1;
        c.lines += conversation.lines.len() as u64;
        self.log_if_notable(&conversation);
        self.conversations.push(conversation);
    }

    /// Logs a quarrel or gossip about a theft, at most every
    /// `conversation_log_hours` per kind.
    fn log_if_notable(&mut self, c: &Conversation) {
        let slot = if c.tone == Tone::Tense {
            0
        } else if c.topic == Topic::Gossip && c.news == Some(News::TheftCaught) {
            1
        } else {
            return;
        };
        let now = self.clock;
        let every = self.params.conversation_log_hours.max(1) * MINUTES_PER_HOUR;
        if self.last_chat_log[slot].is_some_and(|t| now.since(t) < every) {
            return;
        }
        let (Some(a), Some(b)) = (self.npc(c.a), self.npc(c.b)) else {
            return;
        };
        // A quarrel shows in the answer, gossip in the opener.
        let line = c
            .lines
            .get(if slot == 0 { 1 } else { 0 })
            .or(c.lines.first())
            .map(|l| l.text.clone())
            .unwrap_or_default();
        let kind = EventKind::Chat {
            id: c.id,
            npc: a.id,
            name: a.name.clone(),
            other: b.id,
            other_name: b.name.clone(),
            topic: c.topic,
            tone: c.tone,
            about: c.about,
            line,
        };
        self.last_chat_log[slot] = Some(now);
        self.conversation_counters.logged += 1;
        self.push_event(kind);
    }

    // ------------------------------------------------------------------
    // Topic and tone
    // ------------------------------------------------------------------

    /// Relative weights of each topic (indexed like [`Topic::ALL`]) for a
    /// chat started by `a` with `b`: `news` if there is a recent public fact
    /// to tell, `gossip` if there is someone to gossip about, `grievance`
    /// in `0..=1`.
    fn topic_weights(
        &self,
        a: &Npc,
        b: &Npc,
        news: bool,
        gossip: bool,
        grievance: f32,
    ) -> [f32; Topic::COUNT] {
        let (pa, pb) = (a.personality(), b.personality());
        let rel = a.relation(b.id);
        let family = rel.is_some_and(|r| r.kind.is_family());
        let affinity = rel.map_or(0.0, |r| r.affinity);
        let friend = !family && affinity >= GOSSIP_MIN_AFFINITY;
        let stranger = rel.is_none();
        let mut w = [0.0f32; Topic::COUNT];
        let age = a.age.min(b.age);
        if age < BABY_AGE {
            w[if family {
                Topic::Family
            } else {
                Topic::SmallTalk
            }
            .index()] = 1.0;
            return w;
        }
        let adults = age >= LifeStage::ADULTO_FROM;
        let kids = age < KID_AGE;

        let small = if family {
            0.5
        } else if friend {
            0.8
        } else if stranger {
            2.0
        } else {
            1.2
        };
        let shy = if pa.has(Temper::Timido) { 1.4 } else { 1.0 };
        w[Topic::SmallTalk.index()] = small * shy;

        if adults && (a.job.is_some() || b.job.is_some()) {
            let same_job = a.job.is_some() && a.job == b.job;
            let same_place = a.workplace.is_some() && a.workplace == b.workplace;
            w[Topic::Work.index()] = 0.5
                + if same_job { 1.5 } else { 0.0 }
                + if same_place { 0.4 } else { 0.0 }
                + if pa.has(Temper::Chiacchierone) {
                    0.2
                } else {
                    0.0
                };
        }

        let urgency = (1.0 - a.needs.hunger)
            .max(1.0 - a.needs.energy)
            .max(1.0 - a.needs.social);
        w[Topic::Needs.index()] = 4.0 * (urgency - 0.45).max(0.0);

        if news {
            let mut n = if stranger { 0.8 } else { 1.0 };
            if pa.has(Temper::Curioso) {
                n *= 1.5;
            }
            if pa.has(Temper::Pettegolo) {
                n *= 1.2;
            }
            w[Topic::News.index()] = n;
        }

        w[Topic::Family.index()] = match rel.map(|r| r.kind) {
            Some(RelationKind::Partner) => 3.0,
            Some(k) if k.is_family() => 2.5,
            _ if friend && (b.partner().is_some() || b.children().next().is_some()) => 0.5,
            _ => 0.0,
        };

        if gossip && !kids {
            let mut g = if friend {
                1.4
            } else if family {
                0.8
            } else if stranger {
                0.15
            } else {
                0.6
            };
            if pa.has(Temper::Pettegolo) {
                g *= 2.0;
            }
            if pb.has(Temper::Pettegolo) {
                g *= 1.3;
            }
            w[Topic::Gossip.index()] = g;
        }

        if !kids {
            w[Topic::Complaint.index()] = 1.5 * grievance
                + if pa.has(Temper::Lamentoso) { 0.6 } else { 0.0 }
                + if a.needs.hunger < 0.3 { 0.4 } else { 0.0 };
        }
        w
    }

    /// Chooses what NPCs `i` and `j` talk about (uses the world RNG).
    fn plan(&mut self, i: usize, j: usize) -> Plan {
        self.collect_news();
        let now = self.clock;
        let (a, b) = (&self.npcs[i], &self.npcs[j]);
        let window = self.params.news_days.max(1) * MINUTES_PER_DAY;

        // A recent fact they did not live firsthand: the more relevant the likelier.
        let scores: Vec<f32> = self
            .news
            .iter()
            .map(|item| {
                let age = now.since(item.time);
                if age > window || item.involves(a.id) || item.involves(b.id) {
                    return 0.0;
                }
                let base = match item.news {
                    News::TheftCaught | News::Shortage(ItemKind::Razione) => 1.5,
                    News::Death | News::Austerity => 1.2,
                    News::Birth | News::Couple | News::Protest(_) | News::Concession(_) => 1.0,
                    News::Widowed | News::Shortage(_) | News::TheftUnseen => 0.8,
                    News::BirthDenied | News::PayRaised | News::PayCut => 0.7,
                    News::HelpRefused | News::Restocked(_) | News::CameOfAge => 0.5,
                    News::HelpGiven | News::Retired => 0.4,
                };
                let known = |n: &Npc| {
                    item.involved
                        .iter()
                        .flatten()
                        .any(|&x| n.relation(x).is_some())
                };
                let knows = if known(a) {
                    3.0
                } else if known(b) {
                    1.5
                } else {
                    1.0
                };
                let place = item
                    .place
                    .or_else(|| item.home_of.and_then(|h| self.npc(h)).map(|n| n.home));
                let near = match place {
                    Some(p) if p == a.carriage || p == a.home => 1.5,
                    _ => 1.0,
                };
                base * knows * near * (1.0 - 0.7 * age as f32 / window as f32)
            })
            .collect();
        let pick = weighted(&scores, &mut self.rng).map(|k| &self.news[k]);
        // Gossip about a news item needs a subject that `a` knows.
        let gossip_news = pick.filter(|item| {
            matches!(
                item.news,
                News::TheftCaught
                    | News::Couple
                    | News::Birth
                    | News::Widowed
                    | News::HelpRefused
                    | News::HelpGiven
                    | News::CameOfAge
            ) && item.about.is_some()
                && item
                    .involved
                    .iter()
                    .flatten()
                    .any(|&x| a.relation(x).is_some())
        });

        // Generic gossip: someone `a` likes or dislikes, not a child
        // (better if `b` knows them too), picked below if the topic is chosen.
        let npcs = &self.npcs;
        let gossip_weight = |r: &crate::npc::Relation| {
            let grown = index_of(npcs, r.other).is_some_and(|k| npcs[k].age >= KID_AGE);
            if r.other == b.id
                || !grown
                || r.affinity.abs() < GOSSIP_MIN_AFFINITY
                || r.kind == RelationKind::Partner
            {
                0.0
            } else if b.relation(r.other).is_some() {
                1.5 * r.affinity.abs()
            } else {
                r.affinity.abs()
            }
        };
        let generic = gossip_news.is_none() && a.relations.iter().any(|r| gossip_weight(r) > 0.0);

        // Grievances: own denied child, famine, pay cuts.
        let denied = self.news.iter().any(|item| {
            item.news == News::BirthDenied && item.involves(a.id) && now.since(item.time) <= window
        });
        let famine = self.shortages[ItemKind::Razione.index()];
        let money = self.news.iter().find(|item| {
            matches!(item.news, News::Austerity | News::PayCut) && now.since(item.time) <= window
        });
        let (grievance, complaint) = if denied {
            (1.2, Some(News::BirthDenied))
        } else if famine {
            (1.0, Some(News::Shortage(ItemKind::Razione)))
        } else if let Some(item) = money {
            (0.6, Some(item.news))
        } else {
            (0.0, None)
        };

        let public = pick.is_some() && gossip_news.is_none();
        let gossip = gossip_news.is_some() || generic;
        let weights = self.topic_weights(a, b, public, gossip, grievance);
        let topic = weighted(&weights, &mut self.rng).map_or(Topic::SmallTalk, |k| Topic::ALL[k]);

        let need = {
            let n = &a.needs;
            if 1.0 - n.hunger >= (1.0 - n.energy).max(1.0 - n.social) {
                Need::Hunger
            } else if n.energy <= n.social {
                Need::Tiredness
            } else {
                Need::Loneliness
            }
        };
        let mut plan = Plan {
            topic,
            news: None,
            about: None,
            subject: None,
            fond: true,
            need,
        };
        match topic {
            Topic::News => {
                if let Some(item) = pick {
                    plan.news = Some(item.news);
                    plan.about = item.about;
                    plan.subject = self.subject_of(item);
                }
            }
            Topic::Gossip => {
                if let Some(item) = gossip_news {
                    plan.news = Some(item.news);
                    plan.about = item.about;
                    plan.subject = self.subject_of(item);
                } else {
                    let total: f32 = a.relations.iter().map(gossip_weight).sum();
                    let mut r = self.rng.random::<f32>() * total;
                    let chosen = a.relations.iter().find(|rel| {
                        let w = gossip_weight(rel);
                        if w > 0.0 && r < w {
                            return true;
                        }
                        r -= w;
                        false
                    });
                    let chosen =
                        chosen.or_else(|| a.relations.iter().rfind(|rel| gossip_weight(rel) > 0.0));
                    if let Some(rel) = chosen
                        && let Some(n) = self.npc(rel.other)
                    {
                        let other = n
                            .partner()
                            .and_then(|p| self.npc(p))
                            .map_or("", |p| p.name.as_str());
                        plan.about = Some(n.id);
                        plan.subject = Some(subject(&n.name, n.sex, other));
                        plan.fond = rel.affinity > 0.0;
                    }
                }
            }
            Topic::Complaint => plan.news = complaint,
            _ => {}
        }
        plan
    }

    /// Chooses the tone of a conversation between NPCs `i` and `j` about `topic`.
    fn choose_tone(&mut self, i: usize, j: usize, topic: Topic) -> Tone {
        let (a, b) = (&self.npcs[i], &self.npcs[j]);
        let (pa, pb) = (a.personality(), b.personality());
        let affinity = (a.affinity(b.id) + b.affinity(a.id)) / 2.0;
        let compat = pa.compatibility(pb);
        let count = |t: Temper| (u8::from(pa.has(t)) + u8::from(pb.has(t))) as f32;
        let (grumpy, kind, cheerful) = (
            count(Temper::Burbero),
            count(Temper::Gentile),
            count(Temper::Allegro),
        );
        let mut factor = 1.0 + 0.7 * grumpy - 0.3 * kind - 0.2 * cheerful - 0.8 * compat
            + 2.0 * (-affinity).max(0.0)
            - 0.4 * affinity.max(0.0);
        if topic == Topic::Complaint {
            factor += 0.3;
        }
        // Nobody quarrels with a baby.
        let tense = if a.age.min(b.age) < BABY_AGE {
            0.0
        } else {
            (self.params.quarrel_chance * factor.clamp(0.2, 4.0)).clamp(0.0, 0.6)
        };
        let mut friendly =
            0.35 + 0.6 * affinity.max(0.0) + 0.25 * compat + 0.1 * cheerful + 0.1 * kind
                - 0.1 * grumpy;
        if topic == Topic::Family {
            friendly += 0.1;
        }
        let friendly = friendly.clamp(0.1, 0.95);
        let r = self.rng.random::<f32>();
        if r < tense {
            Tone::Tense
        } else if (r - tense) < friendly * (1.0 - tense) {
            Tone::Friendly
        } else {
            Tone::Neutral
        }
    }

    // ------------------------------------------------------------------
    // End
    // ------------------------------------------------------------------

    /// Start of the tick: conversations whose time is up end for both
    /// participants, with their effects.
    pub(super) fn end_conversations(&mut self) {
        let now = self.clock;
        if self.conversations.iter().all(|c| c.until > now) {
            return;
        }
        let mut k = 0;
        while k < self.conversations.len() {
            if self.conversations[k].until <= now {
                let c = self.conversations.remove(k);
                self.conclude(&c);
                self.archive(c);
            } else {
                k += 1;
            }
        }
    }

    /// Effects of a conversation that ran to its end.
    fn conclude(&mut self, c: &Conversation) {
        let (Some(i), Some(j)) = (self.npc_index(c.a), self.npc_index(c.b)) else {
            return;
        };
        // Both stop talking (`finish_action` must not see a one-sided chat).
        for (x, other) in [(i, c.b), (j, c.a)] {
            if self.npcs[x].action == Action::Socialize(other) {
                self.npcs[x].action = Action::Idle;
            }
        }
        // Who talked while eating gets the partner's bonus.
        if matches!(self.npcs[j].action, Action::Eat(_)) {
            let needs = &mut self.npcs[j].needs;
            needs.social = (needs.social + self.params.socialize_partner_bonus).min(1.0);
        }
        let compat = self.npcs[i]
            .personality()
            .compatibility(self.npcs[j].personality());
        let gain = self.params.affinity_per_chat;
        // About 70% friendly, 23% neutral, 7% tense: on average +0.8 × gain,
        // like the old "10% quarrels" chats.
        let delta = match c.tone {
            Tone::Friendly => gain * (1.0 + 0.25 * compat),
            Tone::Neutral => gain * (0.6 + 0.2 * compat),
            Tone::Tense => -gain * (1.0 - 0.25 * compat),
        };
        self.add_affinity(i, j, delta);
        self.spread_gossip(c, i, j);
    }

    /// What `b` (index `j`) heard from `a` (index `i`) changes its opinion
    /// of whom they talked about, if `b` knows them and believed it.
    fn spread_gossip(&mut self, c: &Conversation, i: usize, j: usize) {
        let Some(about) = c.about else {
            return;
        };
        if c.tone == Tone::Tense || !matches!(c.topic, Topic::Gossip | Topic::News) {
            return;
        }
        let g = self.params.gossip_affinity;
        let delta = match c.news {
            Some(News::TheftCaught) => -g,
            Some(News::HelpRefused) => -g / 2.0,
            Some(News::HelpGiven) => g / 2.0,
            Some(News::Couple | News::Birth | News::CameOfAge | News::Widowed) => g * 0.3,
            Some(_) => 0.0,
            // Plain gossip: the listener's opinion moves towards the speaker's.
            None => {
                let (a, b) = (&self.npcs[i], &self.npcs[j]);
                (0.5 * (a.affinity(about) - b.affinity(about))).clamp(-2.0 * g, 2.0 * g)
            }
        };
        if delta == 0.0 {
            return;
        }
        if let Some(r) = self.npcs[j].relation_mut(about) {
            r.affinity = (r.affinity + delta).clamp(-1.0, 1.0);
            self.conversation_counters.gossip_spread += 1;
        }
    }

    /// `id` died: its conversation ends now; the other one stops talking.
    pub(super) fn drop_conversation_of(&mut self, id: NpcId) {
        let Some(pos) = self.conversations.iter().position(|c| c.involves(id)) else {
            return;
        };
        let now = self.clock;
        let mut c = self.conversations.remove(pos);
        if let Some(other) = c.other(id)
            && let Some(k) = self.npc_index(other)
        {
            let npc = &mut self.npcs[k];
            if npc.action == Action::Socialize(id) {
                npc.action = Action::Idle;
                npc.action_until = now;
            }
        }
        c.until = now.max(c.since);
        let until = c.until;
        c.lines.retain(|l| l.at <= until);
        self.conversation_counters.cut_short += 1;
        self.archive(c);
    }

    /// Keeps a finished conversation among the recent ones.
    fn archive(&mut self, c: Conversation) {
        self.recent_conversations.push(c);
        let keep = self.params.recent_conversations_kept;
        if self.recent_conversations.len() > keep {
            let extra = self.recent_conversations.len() - keep;
            self.recent_conversations.drain(..extra);
        }
    }

    // ------------------------------------------------------------------
    // News
    // ------------------------------------------------------------------

    /// Takes the notable facts out of the events logged since the last
    /// call. Runs before the log is trimmed and before choosing a topic.
    pub(super) fn collect_news(&mut self) {
        let total = self.events_total();
        if self.news_seen >= total {
            return;
        }
        let first = self.news_seen.saturating_sub(self.events_dropped) as usize;
        let fresh: Vec<NewsItem> = self.events[first.min(self.events.len())..]
            .iter()
            .filter_map(|e| news_item(e.time, &e.kind))
            .collect();
        self.news.extend(fresh);
        if self.news.len() > NEWS_KEPT {
            let extra = self.news.len() - NEWS_KEPT;
            self.news.drain(..extra);
        }
        self.news_seen = total;
    }

    /// The piece of news NPC `i` would tell the player in the chat, without
    /// randomness: among the recent facts it did not live firsthand, one of
    /// the most relevant few (scored like the topics of a conversation;
    /// `salt` picks which). With whom it is about and where it happened (or
    /// where the NPC is). Call [`World::collect_news`] first.
    pub(super) fn chat_news(
        &self,
        i: usize,
        salt: u64,
    ) -> Option<(News, Option<Subject>, CarriageId)> {
        let now = self.clock;
        let a = &self.npcs[i];
        let window = self.params.news_days.max(1) * MINUTES_PER_DAY;
        let mut scored: Vec<(f32, usize)> = self
            .news
            .iter()
            .enumerate()
            .filter_map(|(k, item)| {
                let age = now.since(item.time);
                if age > window || item.involves(a.id) {
                    return None;
                }
                let known = item
                    .involved
                    .iter()
                    .flatten()
                    .any(|&x| a.relation(x).is_some());
                let place = item
                    .place
                    .or_else(|| item.home_of.and_then(|h| self.npc(h)).map(|n| n.home));
                let near = place.is_some_and(|p| p == a.carriage || p == a.home);
                let score = (1.0 + if known { 2.0 } else { 0.0 } + if near { 0.5 } else { 0.0 })
                    * (1.0 - 0.7 * age as f32 / window as f32);
                Some((score, k))
            })
            .collect();
        if scored.is_empty() {
            return None;
        }
        // Best first; the newest first on a tie.
        scored.sort_by(|x, y| y.0.total_cmp(&x.0).then(y.1.cmp(&x.1)));
        scored.truncate(3);
        let item = &self.news[scored[(salt % scored.len() as u64) as usize].1];
        let place = item
            .place
            .or_else(|| item.home_of.and_then(|h| self.npc(h)).map(|n| n.home))
            .unwrap_or(a.carriage);
        Some((item.news, self.subject_of(item), place))
    }

    /// Whom a news item is about, for the text.
    fn subject_of(&self, item: &NewsItem) -> Option<Subject> {
        if item.name.is_empty() {
            return None;
        }
        let sex = item
            .sex
            .or_else(|| item.about.and_then(|id| self.npc(id)).map(|n| n.sex))
            .unwrap_or(Sex::Male);
        Some(subject(&item.name, sex, &item.other))
    }
}

/// Where a news item happened.
enum Where {
    At(CarriageId),
    HomeOf(NpcId),
    Nowhere,
}

/// Whom a news item is about: full name, sex (if the event says it), partner.
enum Who<'a> {
    Nobody,
    Named(&'a str, Option<Sex>, &'a str),
}

/// The notable fact in an event, if any.
fn news_item(time: GameTime, kind: &EventKind) -> Option<NewsItem> {
    let item = |news, about: Option<NpcId>, involved, place: Where, who: Who| {
        let (name, sex, other) = match who {
            Who::Nobody => (String::new(), None, String::new()),
            Who::Named(name, sex, other) => (name.to_string(), sex, other.to_string()),
        };
        let (place, home_of) = match place {
            Where::At(c) => (Some(c), None),
            Where::HomeOf(id) => (None, Some(id)),
            Where::Nowhere => (None, None),
        };
        Some(NewsItem {
            time,
            news,
            about,
            involved,
            place,
            home_of,
            name,
            sex,
            other,
        })
    };
    let nobody = [None; 3];
    match kind {
        EventKind::Born {
            npc,
            name,
            sex,
            mother,
            father,
            ..
        } => item(
            News::Birth,
            Some(*npc),
            [Some(*npc), Some(*mother), Some(*father)],
            Where::HomeOf(*mother),
            Who::Named(name, Some(*sex), ""),
        ),
        EventKind::NpcDied { npc, name, sex, .. } => item(
            News::Death,
            Some(*npc),
            [Some(*npc), None, None],
            Where::Nowhere,
            Who::Named(name, Some(*sex), ""),
        ),
        EventKind::Coupled {
            npc,
            name,
            partner,
            partner_name,
            moved_to,
        } => item(
            News::Couple,
            Some(*npc),
            [Some(*npc), Some(*partner), None],
            moved_to.map_or(Where::HomeOf(*npc), Where::At),
            Who::Named(name, None, partner_name),
        ),
        EventKind::Widowed {
            npc,
            name,
            sex,
            partner_name,
            ..
        } => item(
            News::Widowed,
            Some(*npc),
            [Some(*npc), None, None],
            Where::HomeOf(*npc),
            Who::Named(name, Some(*sex), partner_name),
        ),
        EventKind::Theft {
            npc,
            name,
            sex,
            carriage,
            caught: true,
            ..
        } => item(
            News::TheftCaught,
            Some(*npc),
            [Some(*npc), None, None],
            Where::At(*carriage),
            Who::Named(name, Some(*sex), ""),
        ),
        // Nobody knows who (the thief keeps quiet about it).
        EventKind::Theft { npc, carriage, .. } => item(
            News::TheftUnseen,
            None,
            [Some(*npc), None, None],
            Where::At(*carriage),
            Who::Nobody,
        ),
        EventKind::HelpAsked {
            npc,
            helper,
            helper_name,
            tokens,
            ..
        } => item(
            if *tokens > 0 {
                News::HelpGiven
            } else {
                News::HelpRefused
            },
            Some(*helper),
            [Some(*helper), Some(*npc), None],
            Where::HomeOf(*helper),
            Who::Named(helper_name, None, ""),
        ),
        EventKind::ProtestCalled {
            grievance, place, ..
        } => item(
            News::Protest(*grievance),
            None,
            nobody,
            Where::At(*place),
            Who::Nobody,
        ),
        EventKind::AdminConceded { grievance, .. } => item(
            News::Concession(*grievance),
            None,
            nobody,
            Where::Nowhere,
            Who::Nobody,
        ),
        EventKind::Shortage { item: what } => item(
            News::Shortage(*what),
            None,
            nobody,
            Where::Nowhere,
            Who::Nobody,
        ),
        EventKind::Restocked { item: what } => item(
            News::Restocked(*what),
            None,
            nobody,
            Where::Nowhere,
            Who::Nobody,
        ),
        EventKind::BirthDenied {
            mother,
            father,
            father_name,
            ..
        } => item(
            News::BirthDenied,
            None,
            [Some(*mother), Some(*father), None],
            Where::HomeOf(*mother),
            Who::Named(father_name, Some(Sex::Male), ""),
        ),
        EventKind::CameOfAge { npc, name, sex, .. } => item(
            News::CameOfAge,
            Some(*npc),
            [Some(*npc), None, None],
            Where::HomeOf(*npc),
            Who::Named(name, Some(*sex), ""),
        ),
        EventKind::Retired { npc, name, sex, .. } => item(
            News::Retired,
            Some(*npc),
            [Some(*npc), None, None],
            Where::HomeOf(*npc),
            Who::Named(name, Some(*sex), ""),
        ),
        EventKind::Austerity { .. } => {
            item(News::Austerity, None, nobody, Where::Nowhere, Who::Nobody)
        }
        EventKind::PayChanged { raised, .. } => item(
            if *raised {
                News::PayRaised
            } else {
                News::PayCut
            },
            None,
            nobody,
            Where::Nowhere,
            Who::Nobody,
        ),
        _ => None,
    }
}
