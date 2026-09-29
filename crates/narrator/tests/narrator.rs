//! The Narratore offline, with a fake model.

use std::sync::Arc;
use std::time::{Duration, Instant};

use llm::{Budget, Llm, LlmError, MockLlm, Replay};
use narrator::{
    Narrator, NarratorConfig, NarratorOutcome, Requested, Verdict, WorldSummary, system_prompt,
};
use sim::World;

const WAIT: Duration = Duration::from_secs(5);

fn summary() -> WorldSummary {
    WorldSummary::from_world(&World::generate(11, 6, 40))
}

fn item_json(name: &str) -> String {
    format!(
        r#"{{"motivo": "Nei Dormitori di coda si gela e le coperte non bastano.", "novita": {{"tipo": "oggetto", "nome": "{name}", "descrizione": "Una sciarpa di tessuto grezzo che tiene un po' di caldo.", "categoria": "durevole", "valore": 9, "pila": 5, "ingredienti": [{{"oggetto": "Tessuto", "qta": 1}}], "lavoro": "Operaio"}}}}"#
    )
}

fn narrator(llm: Arc<dyn Llm>) -> Narrator {
    Narrator::new(NarratorConfig::new(llm, Budget::unlimited()))
}

fn ask(n: &mut Narrator, day: u64) -> NarratorOutcome {
    assert_eq!(n.request(&summary(), day), Requested::Sent);
    n.wait(WAIT).expect("an outcome")
}

#[test]
fn prompt_contains_the_summary_and_the_schema() {
    let s = summary();
    let llm = Arc::new(MockLlm::fixed(item_json("Sciarpa grezza")));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let spy = Arc::new(MockLlm::new(move |r| {
        log.lock().unwrap().push(r.clone());
        llm.complete(r).map(|r| r.text)
    }));
    let mut n = narrator(spy);
    assert_eq!(n.request(&s, 1), Requested::Sent);
    n.wait(WAIT).unwrap();
    let requests = seen.lock().unwrap();
    let r = &requests[0];
    assert!(r.json, "asks for JSON");
    let (system, user) = (&r.messages[0].content, &r.messages[1].content);
    assert_eq!(system, &system_prompt(3));
    for part in [
        "inverno eterno",
        "UNA sola novità",
        "\"tipo\": \"oggetto\"",
        "\"effetto\": \"scorta\"",
        "Esempio",
        "SOLO con un oggetto JSON",
    ] {
        assert!(system.contains(part), "system prompt without {part:?}");
    }
    assert!(
        user.contains(&s.to_json()),
        "user prompt without the summary"
    );
    assert!(user.contains("nessuna"), "no past novelties yet");
}

#[test]
fn a_valid_answer_is_accepted_and_remembered() {
    let seen = Arc::new(std::sync::Mutex::new(String::new()));
    let log = seen.clone();
    let llm = Arc::new(MockLlm::new(move |r| {
        *log.lock().unwrap() = r.messages[1].content.clone();
        Ok(format!(
            "Ecco:\n```json\n{}\n```",
            item_json("Sciarpa grezza")
        ))
    }));
    let mut n = narrator(llm);
    let o = ask(&mut n, 1);
    let Verdict::Accepted(d) = &o.verdict else {
        panic!("{o:?}")
    };
    assert_eq!(d.proposal.name(), "Sciarpa grezza");
    assert_eq!((o.day, o.attempts), (1, 1));
    assert_eq!(n.novelties().len(), 1);
    assert_eq!(n.exchanges()[0].verdict, "accettata");
    // The next prompt lists it, and the same name is now a duplicate.
    let o = ask(&mut n, 2);
    assert!(seen.lock().unwrap().contains("«Sciarpa grezza»"));
    assert!(
        matches!(&o.verdict, Verdict::Rejected { reason, .. } if reason.contains("esiste già")),
        "{o:?}"
    );
}

#[test]
fn a_bad_answer_is_rejected_with_a_reason() {
    let llm = Arc::new(MockLlm::fixed("Propongo una sciarpa, che ne dici?"));
    let mut n = narrator(llm.clone());
    let o = ask(&mut n, 1);
    let Verdict::Rejected { reason, draft, .. } = &o.verdict else {
        panic!("{o:?}")
    };
    assert!(reason.contains("JSON"), "{reason}");
    assert!(draft.is_none());
    assert_eq!(o.attempts, 2);
    assert_eq!(llm.calls(), 2);
    // Valid JSON, wrong shape.
    let llm = Arc::new(MockLlm::fixed(
        r#"{"motivo": "boh", "novita": {"tipo": "magia"}}"#,
    ));
    let o = ask(&mut narrator(llm), 1);
    assert!(
        matches!(&o.verdict, Verdict::Rejected { reason, .. } if reason.contains("formato")),
        "{o:?}"
    );
}

#[test]
fn retries_once_with_the_reason() {
    let llm = Arc::new(MockLlm::new(|r| {
        Ok(if r.messages.len() == 2 {
            // A catalog name: the precheck refuses it.
            item_json("Coperta")
        } else {
            assert_eq!(r.messages.len(), 4);
            let retry = &r.messages[3].content;
            assert!(retry.contains("rifiutata"), "{retry}");
            assert!(retry.contains("«Coperta» esiste già"), "{retry}");
            item_json("Sciarpa grezza")
        })
    }));
    let mut n = narrator(llm.clone());
    let o = ask(&mut n, 3);
    assert!(matches!(o.verdict, Verdict::Accepted(_)), "{o:?}");
    assert_eq!(o.attempts, 2);
    assert_eq!(llm.calls(), 2);
    let verdicts: Vec<&str> = n.exchanges().iter().map(|e| e.verdict.as_str()).collect();
    assert!(
        verdicts[0].starts_with("rifiutata: il nome «Coperta»"),
        "{verdicts:?}"
    );
    assert_eq!(verdicts[1], "accettata");
}

#[test]
fn duplicates_are_rejected() {
    // Case and accents don't hide a duplicate: "TÈ" is the catalog's "tè".
    let llm = Arc::new(MockLlm::fixed(item_json("TÈ")));
    let o = ask(&mut narrator(llm), 1);
    let Verdict::Rejected { reason, draft, .. } = &o.verdict else {
        panic!("{o:?}")
    };
    assert!(reason.contains("esiste già"), "{reason}");
    assert!(draft.is_some(), "the draft parsed");
}

#[test]
fn budget_exhaustion_makes_no_call() {
    let llm = Arc::new(MockLlm::fixed(item_json("Sciarpa grezza")));
    let mut n = Narrator::new(NarratorConfig::new(llm.clone(), Budget::per_hour(0)));
    assert_eq!(n.request(&summary(), 1), Requested::NoBudget);
    assert_eq!(n.poll(), None);
    assert_eq!(n.wait(Duration::from_millis(50)), None);
    assert_eq!(llm.calls(), 0);
    // One call left: the retry can't be made, and that's an outcome too.
    let llm = Arc::new(MockLlm::fixed("niente JSON"));
    let mut n = Narrator::new(NarratorConfig::new(llm.clone(), Budget::per_hour(1)));
    let o = ask(&mut n, 1);
    assert!(
        matches!(&o.verdict, Verdict::Rejected { reason, .. } if reason.contains("budget")),
        "{o:?}"
    );
    assert_eq!(llm.calls(), 1);
}

#[test]
fn never_blocks_and_one_per_day() {
    let llm = Arc::new(
        MockLlm::fixed(item_json("Sciarpa grezza")).with_delay(Duration::from_millis(300)),
    );
    let mut n = narrator(llm);
    let s = summary();
    assert!(n.due(1));
    let start = Instant::now();
    assert_eq!(n.request(&s, 1), Requested::Sent);
    assert!(start.elapsed() < Duration::from_millis(100));
    assert_eq!(n.poll(), None, "not ready yet");
    assert_eq!(n.request(&s, 2), Requested::Busy);
    assert!(!n.due(1));
    assert!(n.wait(WAIT).is_some());
    assert_eq!(n.request(&s, 1), Requested::AlreadyToday);
    assert!(n.due(2));
}

#[test]
fn errors_become_failed_outcomes() {
    let llm = Arc::new(MockLlm::scripted([Err(LlmError::Timeout)]));
    let o = ask(&mut narrator(llm), 1);
    assert_eq!(o.verdict, Verdict::Failed("timed out".to_string()));
}

#[test]
fn record_then_replay_gives_the_same_outcomes() {
    // A model that sometimes fails the precheck, so retries are recorded too.
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let llm = Arc::new(MockLlm::new(move |_| {
        let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(match n {
            0 => item_json("Verdura"),
            1 => item_json("Sciarpa numero uno"),
            2 => item_json("Sciarpa numero due"),
            _ => item_json("Sciarpa numero tre"),
        })
    }));
    let run = |llm: Arc<dyn Llm>| {
        let mut n = narrator(llm);
        let outcomes: Vec<NarratorOutcome> = (0..3).map(|d| ask(&mut n, d)).collect();
        (outcomes, n.take_recording())
    };
    let (live, log) = run(llm);
    assert_eq!(log.len(), 4, "3 days and one retry");
    let json = serde_json::to_string(&log).unwrap();
    let replay = Replay::new(serde_json::from_str::<Vec<llm::RecordedCall>>(&json).unwrap());
    let (again, _) = run(Arc::new(replay));
    assert_eq!(live, again);
    assert_eq!(
        live.iter()
            .filter(|o| matches!(o.verdict, Verdict::Accepted(_)))
            .count(),
        3
    );
}
