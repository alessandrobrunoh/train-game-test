//! The Narratore: asks the LLM for one novelty per game day, in the
//! background, and checks the answer.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use llm::{
    Budget, ConfigError, Llm, LlmConfig, LlmError, LlmQueue, Message, OpenAiClient, RecordedCall,
    Recorder, Request, Response, Ticket, Usage, extract_json, request_key,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::appearance::{COLOURS, DETAILS, SHAPES};
use crate::guard::{EXAMPLE_NAME, Known, MAX_EFFECTS, Rejection, precheck};
use crate::proposal::{Draft, Proposal};
use crate::summary::WorldSummary;

/// How the Narratore talks to the model.
pub struct NarratorConfig {
    /// The model: a real client, a [`llm::MockLlm`] or a [`llm::Replay`].
    pub llm: Arc<dyn Llm>,
    /// Hard cap on calls (a retry is a call too).
    pub budget: Budget,
    /// `None`: the `.env` / provider default.
    pub temperature: Option<f32>,
    pub max_tokens: u32,
    /// Most effects of an event.
    pub max_effects: usize,
    /// Past novelties listed in the prompt (the most recent).
    pub past_shown: usize,
    /// Suggest a kind of novelty, rotating with the day, so the proposals
    /// cover every kind instead of repeating the model's favourite.
    pub suggest_kinds: bool,
}

impl NarratorConfig {
    pub fn new(llm: Arc<dyn Llm>, budget: Budget) -> Self {
        Self {
            llm,
            budget,
            temperature: Some(0.8),
            // A novelty with a panel or a statistic is ≈ 300–600 tokens;
            // reasoning models spend more before answering. A truncated
            // answer is asked again once with twice as many.
            max_tokens: 3000,
            max_effects: MAX_EFFECTS,
            past_shown: 20,
            suggest_kinds: true,
        }
    }

    /// The model configured in `.env` / the environment; `Ok(None)` when the
    /// AI is off. The budget is `LLM_MAX_CALLS_PER_HOUR`.
    pub fn from_env() -> Result<Option<Self>, ConfigError> {
        Ok(LlmConfig::load()?.map(Self::from_llm_config))
    }

    /// A real client for `config`, with its hourly budget and temperature.
    pub fn from_llm_config(config: LlmConfig) -> Self {
        let budget = Budget::per_hour(config.max_calls_per_hour);
        let temperature = config.temperature;
        let mut this = Self::new(Arc::new(OpenAiClient::new(config)), budget);
        this.temperature = temperature.or(this.temperature);
        this
    }
}

/// What [`Narrator::request`] did.
#[derive(Clone, Debug, PartialEq)]
pub enum Requested {
    /// The request is on its way; the outcome arrives through
    /// [`Narrator::poll`].
    Sent,
    /// A request is still in flight.
    Busy,
    /// This day already had its request.
    AlreadyToday,
    /// The hourly budget is spent: no call today.
    NoBudget,
    /// The request couldn't be queued.
    Failed(LlmError),
}

/// How a day's request ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Passed the precheck; waits for the Custode (A2) to be applied.
    Accepted(Draft),
    /// Still invalid after the retry. `draft` is the last answer when it
    /// parsed (the precheck refused it).
    Rejected {
        reason: String,
        draft: Option<Draft>,
        answer: String,
    },
    /// No usable answer: network, timeout, budget for the retry…
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct NarratorOutcome {
    /// Game day of the request.
    pub day: u64,
    pub verdict: Verdict,
    /// Calls made (1 or 2).
    pub attempts: u8,
    /// Sum over the attempts, as measured by the client (or recorded).
    pub latency: Duration,
    pub usage: Usage,
}

/// One call, for measurement: the prompt is in the [`RecordedCall`] with the
/// same `key`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Exchange {
    pub day: u64,
    /// 1 or 2.
    pub attempt: u8,
    /// [`request_key`] of the prompt.
    pub key: String,
    /// The model's text (`None` if the call failed).
    pub answer: Option<String>,
    /// "accettata", "rifiutata: …", "errore: …".
    pub verdict: String,
    pub latency_ms: u64,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Characters of reasoning of a reasoning model (measured, not shown).
    #[serde(default)]
    pub reasoning_chars: u32,
}

/// A novelty accepted so far (shown in the next prompts so the model doesn't
/// repeat itself).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Novelty {
    pub day: u64,
    pub kind: String,
    pub name: String,
    /// The start of its rationale: the problem it answered.
    pub why: String,
}

/// Characters of a rationale kept in [`Novelty::why`].
const WHY_CHARS: usize = 70;

/// Lets the queue and the recorder share one model.
struct Shared(Arc<dyn Llm>);

impl Llm for Shared {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        self.0.complete(request)
    }
}

struct Pending {
    ticket: Ticket,
    day: u64,
    attempt: u8,
    request: Request,
    known: Known,
    latency: Duration,
    usage: Usage,
    /// Already asked again after a truncated answer.
    shortened: bool,
}

/// Asks for one novelty per game day and never blocks the caller:
/// [`Self::request`] queues the call, [`Self::poll`] collects the outcome.
/// Every call is recorded ([`Self::take_recording`]) so a run can be replayed
/// offline with [`llm::Replay`].
pub struct Narrator {
    queue: LlmQueue,
    recorder: Arc<Recorder<Shared>>,
    temperature: Option<f32>,
    max_tokens: u32,
    max_effects: usize,
    past_shown: usize,
    suggest_kinds: bool,
    pending: Option<Pending>,
    outcomes: VecDeque<NarratorOutcome>,
    novelties: Vec<Novelty>,
    accepted: Vec<Proposal>,
    last_day: Option<u64>,
    exchanges: Vec<Exchange>,
}

impl Narrator {
    pub fn new(config: NarratorConfig) -> Self {
        let recorder = Arc::new(Recorder::new(Shared(config.llm)));
        let queue = LlmQueue::new(recorder.clone(), 1, config.budget);
        Self {
            queue,
            recorder,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            max_effects: config.max_effects,
            past_shown: config.past_shown,
            suggest_kinds: config.suggest_kinds,
            pending: None,
            outcomes: VecDeque::new(),
            novelties: Vec::new(),
            accepted: Vec::new(),
            last_day: None,
            exchanges: Vec::new(),
        }
    }

    /// Whether `day` still needs its novelty and nothing is in flight: the
    /// "one novelty per game day" cadence.
    pub fn due(&self, day: u64) -> bool {
        self.pending.is_none() && self.last_day.is_none_or(|d| d < day)
    }

    /// Asks for `day`'s novelty from `summary`, without waiting. At most one
    /// request per day and one in flight; a day refused by the budget is not
    /// asked again.
    pub fn request(&mut self, summary: &WorldSummary, day: u64) -> Requested {
        if self.pending.is_some() {
            return Requested::Busy;
        }
        if self.last_day.is_some_and(|d| d >= day) {
            return Requested::AlreadyToday;
        }
        self.send(summary, day)
    }

    /// Like [`Self::request`] but also on a day that already had its
    /// novelty (a developer's "ask now"): still one in flight, still inside
    /// the budget.
    pub fn request_now(&mut self, summary: &WorldSummary, day: u64) -> Requested {
        if self.pending.is_some() {
            return Requested::Busy;
        }
        self.send(summary, day)
    }

    /// Forgets the novelties so far and remembers `accepted` instead, as
    /// after loading a game: the next prompts list them and their names are
    /// taken. A request in flight is dropped (its answer is ignored);
    /// `last_day` is the last day already asked.
    pub fn restore(
        &mut self,
        accepted: impl IntoIterator<Item = (u64, Draft)>,
        last_day: Option<u64>,
    ) {
        self.novelties.clear();
        self.accepted.clear();
        self.outcomes.clear();
        self.pending = None;
        for (day, draft) in accepted {
            self.remember(day, &draft);
        }
        self.last_day = last_day;
    }

    /// Mean latency of the answered calls so far.
    pub fn mean_latency(&self) -> Option<Duration> {
        let answered: Vec<u64> = self
            .exchanges
            .iter()
            .filter(|e| e.answer.is_some())
            .map(|e| e.latency_ms)
            .collect();
        if answered.is_empty() {
            return None;
        }
        Some(Duration::from_millis(
            answered.iter().sum::<u64>() / answered.len() as u64,
        ))
    }

    fn remember(&mut self, day: u64, draft: &Draft) {
        self.novelties.push(Novelty {
            day,
            kind: draft.proposal.kind_name().to_string(),
            name: draft.proposal.name().to_string(),
            why: short(&draft.rationale, WHY_CHARS),
        });
        self.accepted.push(draft.proposal.clone());
    }

    fn send(&mut self, summary: &WorldSummary, day: u64) -> Requested {
        self.last_day = Some(day.max(self.last_day.unwrap_or(0)));
        let mut known = Known::from_catalog(&summary.catalog);
        for p in &self.accepted {
            known.add(p);
        }
        let stats = self
            .accepted
            .iter()
            .filter(|p| matches!(p, Proposal::Statistic(_)))
            .count();
        let suggestion = self.suggest_kinds.then(|| match suggested_kind(day) {
            // A few statistics are enough: then only when the model wants.
            "statistica" if stats >= MAX_SUGGESTED_STATS => "evento",
            kind => kind,
        });
        let past = &self.novelties[self.novelties.len().saturating_sub(self.past_shown)..];
        let mut request = Request::new(
            system_prompt(self.max_effects),
            user_prompt(summary, past, suggestion),
        )
        .json()
        .max_tokens(self.max_tokens);
        request.temperature = self.temperature;
        match self.queue.submit(request.clone()) {
            Ok(ticket) => {
                self.pending = Some(Pending {
                    ticket,
                    day,
                    attempt: 1,
                    request,
                    known,
                    latency: Duration::ZERO,
                    usage: Usage::default(),
                    shortened: false,
                });
                Requested::Sent
            }
            Err(LlmError::BudgetExceeded) => Requested::NoBudget,
            Err(e) => Requested::Failed(e),
        }
    }

    /// The outcome of a finished request, if any, without waiting.
    pub fn poll(&mut self) -> Option<NarratorOutcome> {
        for done in self.queue.poll() {
            self.handle(done.ticket, done.result);
        }
        self.outcomes.pop_front()
    }

    /// Waits up to `timeout` for an outcome (tools and tests; the game uses
    /// [`Self::poll`]).
    pub fn wait(&mut self, timeout: Duration) -> Option<NarratorOutcome> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(o) = self.poll() {
                return Some(o);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || self.pending.is_none() {
                return None;
            }
            let done = self.queue.wait_next(left)?;
            self.handle(done.ticket, done.result);
        }
    }

    /// Whether a request is in flight.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Novelties accepted so far, oldest first.
    pub fn novelties(&self) -> &[Novelty] {
        &self.novelties
    }

    /// Every call so far, with its verdict and cost.
    pub fn exchanges(&self) -> &[Exchange] {
        &self.exchanges
    }

    /// The calls recorded since the last take (prompt and answer), for
    /// [`llm::Replay`].
    pub fn take_recording(&self) -> Vec<RecordedCall> {
        self.recorder.take_log()
    }

    /// Calls still allowed in the current hour.
    pub fn budget_left(&mut self) -> u32 {
        self.queue.budget_left()
    }

    fn handle(&mut self, ticket: Ticket, result: Result<Response, LlmError>) {
        let Some(mut p) = self.pending.take_if(|p| p.ticket == ticket) else {
            return;
        };
        let key = request_key(&p.request);
        let response = match result {
            Ok(r) => r,
            Err(e) => {
                self.exchanges.push(Exchange {
                    day: p.day,
                    attempt: p.attempt,
                    key,
                    answer: None,
                    verdict: format!("errore: {e}"),
                    latency_ms: 0,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    reasoning_chars: 0,
                });
                // Cut at `max_tokens` (or no text at all): once more, with
                // twice the room and a request to be brief.
                if is_truncation(&e) && !p.shortened {
                    let tokens = p.request.max_tokens.unwrap_or(self.max_tokens);
                    p.request.max_tokens = Some(tokens.saturating_mul(2));
                    p.request.messages.push(Message::user(SHORTER_PROMPT));
                    match self.queue.submit(p.request.clone()) {
                        Ok(ticket) => {
                            p.ticket = ticket;
                            p.attempt += 1;
                            p.shortened = true;
                            self.pending = Some(p);
                        }
                        Err(retry) => {
                            let why = format!(
                                "{} (nuovo tentativo impossibile: {})",
                                describe(&e),
                                describe(&retry)
                            );
                            self.finish(p, Verdict::Failed(why));
                        }
                    }
                    return;
                }
                self.finish(p, Verdict::Failed(describe(&e)));
                return;
            }
        };
        p.latency += response.latency;
        let usage = response.usage.unwrap_or_default();
        p.usage.prompt_tokens += usage.prompt_tokens;
        p.usage.completion_tokens += usage.completion_tokens;
        let parsed = parse(&response.text);
        let checked = parsed.clone().and_then(|d| {
            precheck(&d, &p.known, self.max_effects)?;
            Ok(d)
        });
        self.exchanges.push(Exchange {
            day: p.day,
            attempt: p.attempt,
            key,
            answer: Some(response.text.clone()),
            verdict: match &checked {
                Ok(_) => "accettata".to_string(),
                Err(r) => format!("rifiutata: {r}"),
            },
            latency_ms: response.latency.as_millis() as u64,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            reasoning_chars: response.reasoning_chars,
        });
        match checked {
            Ok(draft) => {
                self.remember(p.day, &draft);
                self.finish(p, Verdict::Accepted(draft));
            }
            Err(reason) if p.attempt == 1 || (p.shortened && p.attempt == 2) => {
                // One retry, with the reason.
                p.request
                    .messages
                    .push(Message::assistant(response.text.clone()));
                p.request
                    .messages
                    .push(Message::user(retry_prompt(&reason)));
                match self.queue.submit(p.request.clone()) {
                    Ok(ticket) => {
                        p.ticket = ticket;
                        p.attempt += 1;
                        self.pending = Some(p);
                    }
                    Err(e) => {
                        let reason = format!("{reason} (nuovo tentativo impossibile: {e})");
                        let verdict = Verdict::Rejected {
                            reason,
                            draft: parsed.ok(),
                            answer: response.text,
                        };
                        self.finish(p, verdict);
                    }
                }
            }
            Err(reason) => {
                let verdict = Verdict::Rejected {
                    reason: reason.0,
                    draft: parsed.ok(),
                    answer: response.text,
                };
                self.finish(p, verdict);
            }
        }
    }

    fn finish(&mut self, p: Pending, verdict: Verdict) {
        self.outcomes.push_back(NarratorOutcome {
            day: p.day,
            verdict,
            attempts: p.attempt,
            latency: p.latency,
            usage: p.usage,
        });
    }
}

/// The draft in a model's answer (the JSON may be wrapped in text or a
/// fence), or why it isn't one.
pub fn parse(answer: &str) -> Result<Draft, Rejection> {
    let json = extract_json(answer)
        .ok_or_else(|| Rejection("nella risposta non c'è un oggetto JSON".to_string()))?;
    let bad = |e: serde_json::Error| {
        Rejection(format!("il JSON non rispetta il formato richiesto ({e})"))
    };
    let mut value: Value = serde_json::from_str(json).map_err(bad)?;
    tidy(&mut value);
    serde_json::from_value(value).map_err(bad)
}

/// Puts back the parts models often write in the wrong place: the panel
/// inside the novelty, the appearance next to it.
fn tidy(value: &mut Value) {
    let Some(top) = value.as_object_mut() else {
        return;
    };
    if !top.contains_key("interfaccia")
        && let Some(panel) = top
            .get_mut("novita")
            .and_then(Value::as_object_mut)
            .and_then(|n| n.remove("interfaccia"))
    {
        top.insert("interfaccia".to_string(), panel);
    }
    if let Some(look) = top.remove("aspetto")
        && let Some(n) = top.get_mut("novita").and_then(Value::as_object_mut)
    {
        n.entry("aspetto").or_insert(look);
    }
}

/// Statistics suggested at most (then the model invents one only when it
/// sees a reason).
pub const MAX_SUGGESTED_STATS: usize = 3;

/// The kind of novelty suggested on `day`, rotating.
pub fn suggested_kind(day: u64) -> &'static str {
    ["oggetto", "evento", "lavoro", "ricetta", "statistica"][(day % 5) as usize]
}

/// The system prompt: tone, rule, schema with an example, "only JSON",
/// then the panel, statistic and appearance whitelists.
pub fn system_prompt(max_effects: usize) -> String {
    let shapes = SHAPES.join(", ");
    let colours = COLOURS.join(", ");
    let details = DETAILS.join(", ");
    format!(
        r#"Sei il Narratore di un gioco ambientato su un treno che corre senza sosta in un inverno eterno: fuori tutto è ghiaccio, dentro sopravvive l'ultima umanità. Come in Snowpiercer, in coda si vive stretti e affamati, verso la testa ci sono privilegi; le risorse scarseggiano e la tensione tra le classi cresce.

Il tuo compito: guarda lo stato del treno e proponi UNA sola novità che risolva un bisogno o una tensione attuale del treno, combinando azioni di base che esistono già (coltivare, cucinare, fabbricare, trasportare, comprare, dare, mangiare, dormire, chiacchierare). Niente magia né tecnologia impossibile: solo ciò che si può fare con gli oggetti, i lavori e le carrozze del catalogo.

Tipi di novità:
- "oggetto": un oggetto nuovo, fatto da un lavoro esistente con oggetti esistenti.
- "ricetta": un modo nuovo di produrre un oggetto esistente.
- "lavoro": un mestiere nuovo in un tipo di carrozza esistente, che produce oggetti esistenti.
- "evento": un fatto che succede una volta e cambia le scorte o i bisogni (da 1 a {max_effects} effetti).
- "statistica": un numero nuovo, calcolato dal treno, per una tensione che nessun numero misura.

Regole:
- Nomi in italiano, brevi (da 2 a 32 caratteri, al massimo 4 parole, solo lettere), mai uguali a un nome del catalogo o a una novità già proposta. Non copiare l'esempio.
- Oggetti, lavori e carrozze si citano con il loro nome esatto del catalogo (o di una novità già accettata).
- Numeri interi: "valore" 1-200 gettoni, "pila" 1-50, "qta" 1-10, "minuti" 10-480, "delta" di una scorta tra -50 e 50 (mai 0). Il "delta" di un bisogno è un decimale tra -0.3 e 0.3 (mai 0; positivo = meglio).
- "motivo": una o due frasi sul bisogno o la tensione del treno a cui rispondi. "descrizione": una o due frasi. Motivo e descrizione al massimo 200 caratteri, un pannello di solito 2-4 elementi.
- Rispondi SOLO con un oggetto JSON compatto, senza spazi superflui, senza testo prima o dopo e senza commenti.

Formato: {{"motivo": string, "novita": NOVITA, "interfaccia"?: PANNELLO}}, dove NOVITA è uno di:
{{"tipo": "oggetto", "nome": string, "descrizione": string, "categoria": "materia_prima" | "semilavorato" | "consumabile" | "durevole", "valore": int, "pila": int, "ingredienti": [{{"oggetto": string, "qta": int}}], "lavoro": string, "aspetto": {{"forma": FORMA, "colore": COLORE, "dettaglio"?: DETTAGLIO}}}}
{{"tipo": "ricetta", "nome": string, "prodotto": string, "qta": int, "ingredienti": [{{"oggetto": string, "qta": int}}], "lavoro": string, "minuti": int}}
{{"tipo": "lavoro", "nome": string, "descrizione": string, "carrozza": string, "produce": [string]}}
{{"tipo": "evento", "titolo": string, "descrizione": string, "effetti": [EFFETTO]}}
{{"tipo": "statistica", ...}}: come nell'esempio in fondo ("unita" e "soglie" facoltative; ogni soglia ha "sotto" o "sopra").
EFFETTO è uno di:
{{"effetto": "scorta", "carrozza": string, "oggetto": string, "delta": int}}
{{"effetto": "bisogno", "carrozza": string | null, "bisogno": "sazieta" | "energia" | "socialita", "delta": number}}
FORMULA: {{"media"|"somma"|"minimo"|"massimo":[{{"peso":n,"sorgente":S}}]}} (1-6 termini; media = somma di peso×valore / termini) o {{"sorgente":S}}.
FORMA: {shapes}. COLORE: {colours}. DETTAGLIO: {details}.
PANNELLO (aggiungilo se aiuta il giocatore a seguire o usare la novità): {{"titolo":string,"elementi":[E] (1-8)}}, E: {{"tipo":"testo","testo"}} | {{"tipo":"valore","etichetta","sorgente":S,"formato":"numero"|"percento"|"gettoni"}} | {{"tipo":"barra","etichetta","sorgente":S,"min","max"}} | {{"tipo":"lista","etichetta","sorgente_lista":L}} | {{"tipo":"pulsante","etichetta","azione":A}}
S: {{"scorta"|"riempimento":oggetto,"carrozza"?:tipo}} | {{"prezzo":oggetto,"mercato"?:"economico"|"vicino"}} | {{"lavoratori":lavoro}} | {{"popolazione":"tutti"|"bambini"|"giovani"|"adulti"|"anziani"}} | {{"bisogno":"sazieta"|"energia"|"socialita","carrozza"?:tipo}} | {{"economia":"tesoro"|"disuguaglianza"|"gettoni_mediani"}} | {{"giocatore":"gettoni"|"affinita"|oggetto}} | {{"tempo":"giorno"|"ora"}} | {{"statistica":nome}}; bisogno, riempimento e disuguaglianza da 0 a 1.
L: {{"lavoratori":lavoro}} | {{"carrozze_con":oggetto}} | {{"affamati":"treno"|tipo}} | {{"amici":"giocatore"}} | {{"prezzi":oggetto}}
A: {{"apri_crafting":ricetta o oggetto}} | {{"apri_mercato":oggetto}} | {{"apri_inventario":true}} | {{"parla_con_lavoro":lavoro}} | {{"mostra_statistica":nome}} | {{"vai_a":tipo di carrozza}}
S, L e A citano nomi esistenti o la novità stessa.

Esempio di risposta:
{{"motivo": "Le coperte scarseggiano e nei Dormitori di coda si gela.", "novita": {{"tipo": "oggetto", "nome": "{EXAMPLE_NAME}", "descrizione": "Uno scialle cucito con ritagli di tessuto: poco caldo, ma meglio di niente.", "categoria": "durevole", "valore": 12, "pila": 5, "ingredienti": [{{"oggetto": "tessuto", "qta": 2}}], "lavoro": "operaio", "aspetto": {{"forma": "stoffa", "colore": "marrone", "dettaglio": "toppa"}}}}, "interfaccia": {{"titolo": "Freddo in coda", "elementi": [{{"tipo": "barra", "etichetta": "Energia", "sorgente": {{"bisogno": "energia", "carrozza": "Dormitorio"}}, "min": 0, "max": 1}}, {{"tipo": "pulsante", "etichetta": "Cuci uno scialle", "azione": {{"apri_crafting": "{EXAMPLE_NAME}"}}}}]}}}}
Esempio di statistica (la "novita"):
{{"tipo": "statistica", "nome": "Morale", "descrizione": "L'animo del treno.", "unita": "%", "scala": [0, 100], "formula": {{"media": [{{"peso": 100, "sorgente": {{"bisogno": "sazieta"}}}}, {{"peso": 100, "sorgente": {{"bisogno": "socialita"}}}}]}}, "soglie": [{{"sotto": 30, "testo": "Il treno è allo stremo"}}]}}"#
    )
}

/// The user prompt: the summary, the past novelties, the suggested kind.
pub fn user_prompt(summary: &WorldSummary, past: &[Novelty], suggestion: Option<&str>) -> String {
    let mut out = format!(
        "Stato del treno, giorno {} ore {} (JSON):\n{}\n\nNovità già proposte (non ripeterle e non riusarne i nomi):\n",
        summary.day,
        summary.time,
        summary.to_json()
    );
    if past.is_empty() {
        out.push_str("- nessuna\n");
    }
    for n in past {
        let _ = writeln!(
            out,
            "- giorno {}: {} «{}» (per: {})",
            n.day, n.kind, n.name, n.why
        );
    }
    out.push('\n');
    if !past.is_empty() {
        out.push_str(
            "Se un problema ha già avuto una novità, preferisci un altro bisogno o un'altra tensione del treno.\n",
        );
    }
    if let Some(kind) = suggestion {
        let _ = writeln!(
            out,
            "Oggi preferibilmente proponi {} {kind}, se ha senso per il treno.",
            if matches!(kind, "ricetta" | "statistica") {
                "una"
            } else {
                "un"
            }
        );
    }
    out.push_str("Proponi la novità di oggi. Solo JSON.");
    out
}

/// The first `max` characters of `text`, with "…" if cut.
fn short(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// The follow-up after an answer cut at `max_tokens`.
pub const SHORTER_PROMPT: &str = "La risposta precedente era troppo lunga ed è stata troncata: sii più breve. JSON compatto, descrizioni brevi, al massimo 4 elementi nel pannello.";

/// Whether `e` means the answer was cut (or came without text).
fn is_truncation(e: &LlmError) -> bool {
    match e {
        LlmError::Truncated => true,
        LlmError::BadResponse(why) => why.contains("no text in the answer"),
        _ => false,
    }
}

/// Why a call failed, in Italian, for the chronicle.
pub fn describe(e: &LlmError) -> String {
    match e {
        LlmError::Truncated => "risposta troncata (il modello ha finito i token)".to_string(),
        LlmError::BadResponse(why) if why.contains("no text in the answer") => {
            "risposta vuota".to_string()
        }
        LlmError::BadResponse(why) => format!("risposta non valida ({why})"),
        LlmError::Timeout => "il modello non ha risposto in tempo".to_string(),
        LlmError::Http { status, .. } => format!("errore HTTP {status} dal modello"),
        LlmError::Transport(why) => format!("modello non raggiungibile ({why})"),
        LlmError::BudgetExceeded => "budget orario di chiamate esaurito".to_string(),
        LlmError::NotRecorded => "nessuna risposta registrata".to_string(),
    }
}

/// The follow-up after a rejected answer.
pub fn retry_prompt(reason: &Rejection) -> String {
    format!(
        "La proposta è stata rifiutata: {reason}. Correggila e rispondi di nuovo con il solo JSON nel formato richiesto."
    )
}
