//! Conversazioni tra NPC: chi parla con chi, di cosa, e le battute dette.
//!
//! Una chiacchierata (`Action::Socialize`) coinvolge due NPC nello stesso
//! posto: la sim apre una [`Conversation`] con un [`Topic`], ne scrive le
//! battute in italiano distribuite nel tempo della chiacchierata, e alla fine
//! ne applica gli effetti (affinità, notizie, pettegolezzi).
//!
//! Le battute seguono un filo solo (vedi `text`): saluto, l'argomento
//! (la notizia, il lavoro, la famiglia…), una risposta coerente con com'è
//! la notizia (bella, brutta, scandalosa) e con il tono, un seguito sullo
//! stesso argomento e i saluti; il carattere cambia le parole, non
//! l'argomento. [`grammar`] mette preposizioni e articoli giusti davanti ai
//! nomi delle carrozze ("all'Alveare", "nella Brace") e la d eufonica
//! ("Bruno ed Elena").
//!
//! La chat del giocatore ([`crate::chat`]) prende le risposte da [`chat`]:
//! intenzione × affinità × carattere × stato dell'NPC.
//!
//! Il `game` legge [`World::conversations`](crate::World::conversations) per
//! disegnare i fumetti: a ogni istante mostra la battuta con `at` più recente
//! non successiva all'ora corrente.

use serde::{Deserialize, Serialize};

use crate::CarriageId;
use crate::deliberation::Grievance;
use crate::ids::NpcId;
use crate::item::ItemKind;
use crate::time::GameTime;

pub(crate) mod chat;
pub mod grammar;
pub(crate) mod text;

pub use text::{MAX_LINE_CHARS, template_count};

/// Identificativo di una conversazione: crescente, mai riusato.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub u64);

/// Di cosa si parla.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Topic {
    /// Saluti e chiacchiere di circostanza.
    SmallTalk,
    /// Il lavoro, i turni, gli attrezzi.
    Work,
    /// Fame, stanchezza, solitudine: come si sta.
    Needs,
    /// Un fatto recente del treno (nascita, morte, carenza, protesta…).
    News,
    /// Famiglia: partner, figli, genitori.
    Family,
    /// Pettegolezzo su un terzo (vedi `Conversation::about`).
    Gossip,
    /// Lamentele: razioni, divieto di nascite, prezzi.
    Complaint,
}

impl Topic {
    pub const COUNT: usize = 7;
    pub const ALL: [Topic; Self::COUNT] = [
        Topic::SmallTalk,
        Topic::Work,
        Topic::Needs,
        Topic::News,
        Topic::Family,
        Topic::Gossip,
        Topic::Complaint,
    ];

    /// Position in [`Topic::ALL`] (and in per-topic arrays).
    pub fn index(self) -> usize {
        self as usize
    }

    /// Short Italian name: "chiacchiere", "lavoro", "pettegolezzi"...
    pub fn name(self) -> &'static str {
        match self {
            Topic::SmallTalk => "chiacchiere",
            Topic::Work => "lavoro",
            Topic::Needs => "bisogni",
            Topic::News => "notizie",
            Topic::Family => "famiglia",
            Topic::Gossip => "pettegolezzi",
            Topic::Complaint => "lamentele",
        }
    }

    /// What people talk about, after "chiacchiera con Marta": "di lavoro".
    pub fn about_phrase(self) -> &'static str {
        match self {
            Topic::SmallTalk => "del più e del meno",
            Topic::Work => "di lavoro",
            Topic::Needs => "di come si sente",
            Topic::News => "delle ultime notizie",
            Topic::Family => "di famiglia",
            Topic::Gossip => "di pettegolezzi",
            Topic::Complaint => "per lamentarsi",
        }
    }
}

/// Tono della conversazione (colore del fumetto, effetto sull'affinità).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tone {
    Friendly,
    Neutral,
    Tense,
}

impl Tone {
    pub const COUNT: usize = 3;
    pub const ALL: [Tone; Self::COUNT] = [Tone::Friendly, Tone::Neutral, Tone::Tense];

    pub fn index(self) -> usize {
        self as usize
    }

    /// "cordiale", "neutro", "teso".
    pub fn name(self) -> &'static str {
        match self {
            Tone::Friendly => "cordiale",
            Tone::Neutral => "neutro",
            Tone::Tense => "teso",
        }
    }
}

/// A fact of the train people talk about (from the event log, see
/// [`Conversation::news`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum News {
    Birth,
    Death,
    Couple,
    Widowed,
    /// A thief was caught (`about` is the thief).
    TheftCaught,
    /// Something vanished from a Mercato, nobody knows who took it.
    TheftUnseen,
    /// `about` helped someone with tokens...
    HelpGiven,
    /// ...or refused to.
    HelpRefused,
    Protest(Grievance),
    Concession(Grievance),
    Shortage(ItemKind),
    Restocked(ItemKind),
    /// The administration refused a couple a child.
    BirthDenied,
    CameOfAge,
    Retired,
    Austerity,
    PayRaised,
    PayCut,
    /// `about` attacked someone (the subject's `other`), see [`crate::combat`].
    Fight,
    /// `about` killed someone (the subject's `other`).
    Killing,
    /// `about` leads a new gang (the subject's `other` is the gang after
    /// "di": "dei Topi della Coda"), see [`crate::gang`].
    GangFounded,
    /// A gang (`other`) beat `about`, who would not pay the pizzo.
    GangBeating,
    /// A gang (`other`) had `about` killed.
    GangHit,
}

impl News {
    /// Whether the news is good, bad or scandalous (it shapes the answers).
    pub fn valence(self) -> Valence {
        match self {
            News::Birth
            | News::Couple
            | News::HelpGiven
            | News::Concession(_)
            | News::Restocked(_)
            | News::CameOfAge
            | News::Retired
            | News::PayRaised => Valence::Good,
            News::Death
            | News::Widowed
            | News::Protest(_)
            | News::Shortage(_)
            | News::BirthDenied
            | News::Austerity
            | News::PayCut
            | News::GangFounded => Valence::Bad,
            News::TheftCaught
            | News::TheftUnseen
            | News::HelpRefused
            | News::Fight
            | News::Killing
            | News::GangBeating
            | News::GangHit => Valence::Scandal,
        }
    }
}

/// How a piece of news (or gossip) sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Valence {
    Good,
    Bad,
    Scandal,
}

/// Una battuta detta da `speaker` all'ora `at`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub speaker: NpcId,
    pub at: GameTime,
    /// Testo breve in italiano, adatto a un fumetto (al massimo ~40 caratteri).
    pub text: String,
}

/// Una conversazione in corso o conclusa.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    /// Chi ha iniziato.
    pub a: NpcId,
    /// L'interlocutore.
    pub b: NpcId,
    pub carriage: CarriageId,
    pub since: GameTime,
    pub until: GameTime,
    pub topic: Topic,
    /// Di chi si parla, per `Topic::Gossip` e per le notizie su qualcuno.
    pub about: Option<NpcId>,
    pub tone: Tone,
    /// Battute in ordine di tempo, tutte tra `since` e `until`.
    pub lines: Vec<Line>,
    /// The fact talked about (News, Gossip or Complaint), if any.
    #[serde(default)]
    pub news: Option<News>,
}

impl Conversation {
    /// La battuta da mostrare all'ora `now`: l'ultima con `at <= now`.
    pub fn line_at(&self, now: GameTime) -> Option<&Line> {
        self.lines.iter().take_while(|l| l.at <= now).last()
    }

    /// L'altro partecipante, se `npc` è uno dei due.
    pub fn other(&self, npc: NpcId) -> Option<NpcId> {
        if npc == self.a {
            Some(self.b)
        } else if npc == self.b {
            Some(self.a)
        } else {
            None
        }
    }

    pub fn involves(&self, npc: NpcId) -> bool {
        self.a == npc || self.b == npc
    }

    /// Length in game minutes.
    pub fn minutes(&self) -> u64 {
        self.until.since(self.since)
    }
}

/// Conversation counters since the world was generated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationCounters {
    /// Chats started (`Action::Socialize`): two-sided conversations plus
    /// one-sided chats with a busy partner.
    pub chats: u64,
    /// Two-sided conversations opened (see [`crate::World::conversations`]).
    pub conversations: u64,
    /// Of which with a partner who kept eating.
    pub while_eating: u64,
    /// One-sided chats: the partner was busy (working, asleep, away, in
    /// another conversation) or turned away.
    pub one_sided: u64,
    /// Conversations ended early by a death.
    pub cut_short: u64,
    /// Per [`Topic::index`] and [`Tone::index`].
    pub by_topic: [u64; Topic::COUNT],
    pub by_tone: [u64; Tone::COUNT],
    /// Lines written.
    pub lines: u64,
    /// Gossip that changed someone's opinion of a third person.
    pub gossip_spread: u64,
    /// Notable conversations logged as events.
    pub logged: u64,
}

impl ConversationCounters {
    /// Share of the chats that were two-sided conversations.
    pub fn two_sided_share(&self) -> f32 {
        self.conversations as f32 / self.chats.max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv() -> Conversation {
        let line = |s: u32, m: u64, t: &str| Line {
            speaker: NpcId(s),
            at: GameTime(m),
            text: t.to_string(),
        };
        Conversation {
            id: ConversationId(1),
            a: NpcId(1),
            b: NpcId(2),
            carriage: CarriageId(0),
            since: GameTime(100),
            until: GameTime(120),
            topic: Topic::SmallTalk,
            about: None,
            tone: Tone::Friendly,
            lines: vec![line(1, 100, "Ciao!"), line(2, 105, "Come va?")],
            news: None,
        }
    }

    #[test]
    fn line_at_picks_latest_spoken_line() {
        let c = conv();
        assert_eq!(c.line_at(GameTime(99)), None);
        assert_eq!(
            c.line_at(GameTime(100)).map(|l| l.text.as_str()),
            Some("Ciao!")
        );
        assert_eq!(
            c.line_at(GameTime(119)).map(|l| l.text.as_str()),
            Some("Come va?")
        );
    }

    #[test]
    fn other_and_involves() {
        let c = conv();
        assert_eq!(c.other(NpcId(1)), Some(NpcId(2)));
        assert_eq!(c.other(NpcId(3)), None);
        assert!(c.involves(NpcId(2)) && !c.involves(NpcId(3)));
    }
}
