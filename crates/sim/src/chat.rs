//! Chat del giocatore con gli NPC (Fase 5.4 di `docs/piano-vita-ed-economia.md`).
//!
//! The player talks to an NPC by choosing an [`Intent`] (the suggested
//! replies: greet, ask about work, prices, a favour, give, trade, news,
//! insult, say goodbye) or by typing free text, which an [`IntentReader`]
//! maps to one of them ([`KeywordReader`]: deterministic keywords with
//! tolerance for accents and typos; an LLM reader can replace it later).
//!
//! The world applies a chat at the current minute
//! ([`crate::World::player_chat`], [`crate::World::player_chat_text`]), like
//! the other `player_*` calls: no randomness, so the simulation stays
//! deterministic given the same calls at the same minutes. The NPC's answer
//! comes from [`crate::dialogue`] (intent × [`Band`] × personality × the NPC's
//! state), and both lines are kept in the NPC's [`ChatLog`] (the last
//! [`CHAT_MEMORY_LINES`], saved with the world in
//! [`crate::PlayerCharacter::chats`]).
//!
//! Effects (see `world/chat.rs`): a little affinity for a greeting (at most
//! once every [`CHAT_BONUS_COOLDOWN_MINUTES`]), less for an insult (at most
//! once every [`INSULT_COOLDOWN_MINUTES`]); a [`Favour`] asked by the NPC
//! ("bring me 2 verdure"), paid on delivery from the NPC's own tokens.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{CarriageId, NpcId};
use crate::item::ItemKind;
use crate::player::{PlayerTie, Regard};
use crate::time::GameTime;

mod reader;

pub use reader::{IntentReader, KeywordReader, normalize};

/// Lines kept per NPC in its [`ChatLog`] (both speakers).
pub const CHAT_MEMORY_LINES: usize = 24;
/// NPCs whose chat is remembered (the least recently talked to are forgotten).
pub const CHAT_LOGS_KEPT: usize = 96;
/// Longest line the player can type (characters; the rest is cut).
pub const MAX_CHAT_INPUT_CHARS: usize = 120;
/// Longest line said in a chat (characters): the NPC's answers stay below.
pub const MAX_CHAT_LINE_CHARS: usize = 90;
/// Affinity gained by a greeting (at most once every
/// [`CHAT_BONUS_COOLDOWN_MINUTES`]).
pub const CHAT_GREET_AFFINITY: f32 = 0.03;
/// Minutes between two greetings that raise the affinity.
pub const CHAT_BONUS_COOLDOWN_MINUTES: u64 = 6 * 60;
/// Affinity lost for an insult (at most once every [`INSULT_COOLDOWN_MINUTES`]).
pub const INSULT_AFFINITY: f32 = -0.15;
/// Minutes before another insult lowers the affinity again.
pub const INSULT_COOLDOWN_MINUTES: u64 = 60;
/// Affinity gained when the player does a favour.
pub const FAVOUR_AFFINITY: f32 = 0.15;
/// Days a favour stays open.
pub const FAVOUR_DAYS: u64 = 2;
/// An NPC offers the player a favour on its own (with a "!") at most once in
/// this many minutes.
pub const FAVOUR_OFFER_COOLDOWN_MINUTES: u64 = 24 * 60;
/// At most this many open favours at once, over all the NPCs.
pub const MAX_OPEN_FAVOURS: usize = 3;
/// Share of its tokens an NPC is willing to promise for a favour.
pub const FAVOUR_MAX_TOKEN_SHARE: f32 = 0.5;

/// What the player means: the suggested replies of the chat window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Intent {
    Greet,
    AskJob,
    AskPrices,
    AskFavour,
    Gift,
    Trade,
    AskNews,
    Insult,
    Farewell,
}

impl Intent {
    pub const ALL: [Intent; 9] = [
        Intent::Greet,
        Intent::AskJob,
        Intent::AskPrices,
        Intent::AskFavour,
        Intent::Gift,
        Intent::Trade,
        Intent::AskNews,
        Intent::Insult,
        Intent::Farewell,
    ];

    /// Label of the suggested reply: "Saluta", "Chiedi del lavoro"...
    pub fn label(self) -> &'static str {
        match self {
            Intent::Greet => "Saluta",
            Intent::AskJob => "Chiedi del lavoro",
            Intent::AskPrices => "Chiedi dei prezzi",
            Intent::AskFavour => "Chiedi un favore",
            Intent::Gift => "Regala",
            Intent::Trade => "Scambia",
            Intent::AskNews => "Chiedi notizie",
            Intent::Insult => "Insulta",
            Intent::Farewell => "Congedati",
        }
    }

    /// One-line explanation (tooltips).
    pub fn describe(self) -> &'static str {
        match self {
            Intent::Greet => "Un saluto: un po' di simpatia, non più di una volta ogni tanto.",
            Intent::AskJob => "Che lavoro fa, dove e quando.",
            Intent::AskPrices => "Dove costa meno quello che gli interessa, per quel che sa.",
            Intent::AskFavour => "Un incarico: portargli qualcosa, pagato con i suoi gettoni.",
            Intent::Gift => "Apre la scelta del regalo.",
            Intent::Trade => "Un mercante apre il suo banco; gli altri ti indirizzano.",
            Intent::AskNews => "Le ultime notizie del treno, e cosa si dice di te.",
            Intent::Insult => "Lo offende: la simpatia cala.",
            Intent::Farewell => "Chiude la chiacchierata.",
        }
    }
}

impl fmt::Display for Intent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// How well the NPC likes the player, for the answers: below
/// [`Regard::WARY_BELOW`] it is low, from [`Regard::FRIEND_FROM`] high.
/// Who never met the player is in the middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Band {
    Low,
    Mid,
    High,
}

impl Band {
    pub const ALL: [Band; 3] = [Band::Low, Band::Mid, Band::High];

    pub fn of_affinity(affinity: f32) -> Band {
        if affinity < Regard::WARY_BELOW {
            Band::Low
        } else if affinity >= Regard::FRIEND_FROM {
            Band::High
        } else {
            Band::Mid
        }
    }

    pub fn of(tie: Option<&PlayerTie>) -> Band {
        tie.map_or(Band::Mid, |t| Band::of_affinity(t.affinity))
    }
}

/// Who said a chat line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Speaker {
    Player,
    Npc,
}

/// One line of a chat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLine {
    pub speaker: Speaker,
    pub at: GameTime,
    pub text: String,
}

/// What the player and one NPC said to each other, the last
/// [`CHAT_MEMORY_LINES`] lines, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLog {
    pub npc: NpcId,
    pub lines: Vec<ChatLine>,
    /// Lines ever said (also those forgotten): it varies the wording.
    #[serde(default)]
    pub said: u64,
}

impl ChatLog {
    /// When they last talked.
    pub fn last(&self) -> Option<GameTime> {
        self.lines.last().map(|l| l.at)
    }

    pub(crate) fn push(&mut self, speaker: Speaker, at: GameTime, text: String) {
        self.lines.push(ChatLine { speaker, at, text });
        self.said += 1;
        if self.lines.len() > CHAT_MEMORY_LINES {
            let extra = self.lines.len() - CHAT_MEMORY_LINES;
            self.lines.drain(..extra);
        }
    }
}

/// A small errand an NPC asks the player: bring `count` × `item` by
/// `until`, for `reward` tokens taken from the NPC's own (never minted).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favour {
    pub item: ItemKind,
    pub count: u32,
    /// Tokens promised; on delivery the NPC pays what it still has, up to this.
    pub reward: u32,
    pub since: GameTime,
    pub until: GameTime,
    /// The NPC already asked the player. False while it is waiting to tell
    /// (it shows a "!", see [`crate::PlayerTie::wants_to_talk`]).
    pub told: bool,
}

/// What the game should do after an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatAction {
    None,
    /// Show the gift choice (the NPC accepts gifts).
    OpenGift,
    /// Open the market window on this Mercato (the NPC is its Mercante, at the counter).
    OpenMarket(CarriageId),
    /// The chat is over (goodbye).
    End,
}

/// The outcome of one exchange: what the player said, the NPC's answer and
/// the effects applied.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatReply {
    /// What the player meant (None: the NPC did not understand).
    pub intent: Option<Intent>,
    /// The player's line, as recorded.
    pub said: String,
    /// The NPC's answer.
    pub answer: String,
    /// Change of the NPC's affinity with the player.
    pub affinity: f32,
    /// Tokens the NPC paid the player (a favour done).
    pub tokens: u32,
    /// A favour asked (new or reminded) or done by this exchange.
    pub favour: Option<Favour>,
    pub action: ChatAction,
}

/// Why the player can't chat with an NPC (shown after its name: "Marta non è qui").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatError {
    NoSuchNpc,
    /// The NPC is not where the player is.
    Away,
    /// The NPC is asleep.
    Asleep,
    /// The player is asleep.
    PlayerAsleep,
}

impl fmt::Display for ChatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChatError::NoSuchNpc => f.write_str("non c'è più"),
            ChatError::Away => f.write_str("non è qui"),
            ChatError::Asleep => f.write_str("sta dormendo"),
            ChatError::PlayerAsleep => f.write_str("non ti sente: stai dormendo"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_follow_the_regard_thresholds() {
        assert_eq!(Band::of(None), Band::Mid);
        assert_eq!(Band::of_affinity(-0.5), Band::Low);
        assert_eq!(Band::of_affinity(0.0), Band::Mid);
        assert_eq!(Band::of_affinity(Regard::FRIEND_FROM), Band::High);
        let labels: Vec<&str> = Intent::ALL.iter().map(|i| i.label()).collect();
        assert_eq!(labels.len(), 9);
        assert!(Intent::ALL.iter().all(|i| !i.describe().is_empty()));
    }

    #[test]
    fn logs_keep_the_last_lines() {
        let mut log = ChatLog {
            npc: NpcId(1),
            lines: Vec::new(),
            said: 0,
        };
        for k in 0..(CHAT_MEMORY_LINES as u64 + 5) {
            log.push(Speaker::Player, GameTime(k), format!("riga {k}"));
        }
        assert_eq!(log.lines.len(), CHAT_MEMORY_LINES);
        assert_eq!(log.lines[0].text, "riga 5");
        assert_eq!(log.said, CHAT_MEMORY_LINES as u64 + 5);
        assert_eq!(log.last(), Some(GameTime(CHAT_MEMORY_LINES as u64 + 4)));
    }
}
