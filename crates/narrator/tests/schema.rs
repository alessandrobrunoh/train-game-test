//! Panels, statistics and appearances: validation, read-only evaluation on a
//! generated World, the prompt's whitelists, and a mock round trip.

use std::sync::Arc;
use std::time::Duration;

use llm::{Budget, MockLlm};
use narrator::appearance::{COLOURS, DETAILS, SHAPES};
use narrator::guard::MAX_EFFECTS;
use narrator::panel::{ACTION_KEYS, ELEMENT_KINDS};
use narrator::sources::{LIST_KEYS, SOURCE_KEYS};
use narrator::{
    Catalog, Draft, Known, Narrator, NarratorConfig, Proposal, Reading, Requested, Source,
    StatBook, Statistic, Verdict, WorldSummary, parse, precheck, system_prompt,
};
use sim::{CarriageKind, ItemKind, Job, UtilityBrain, World};

fn known() -> Known {
    Known::from_catalog(&Catalog::of_sim())
}

fn why(json: &str) -> String {
    let d = parse(json).unwrap_or_else(|e| panic!("{e}: {json}"));
    precheck(&d, &known(), MAX_EFFECTS).expect_err("rejected").0
}

fn ok(json: &str) -> Draft {
    let d = parse(json).unwrap_or_else(|e| panic!("{e}: {json}"));
    if let Err(e) = precheck(&d, &known(), MAX_EFFECTS) {
        panic!("{e}: {json}")
    }
    d
}

const ITEM: &str = r#""novita": {"tipo": "oggetto", "nome": "Borraccia", "descrizione": "Una borraccia di latta battuta.", "categoria": "durevole", "valore": 9, "pila": 3, "ingredienti": [{"oggetto": "metallo", "qta": 1}], "lavoro": "operaio""#;

fn item_with(appearance: &str, panel: &str) -> String {
    format!(r#"{{"motivo": "Si beve poco e male in coda.", {ITEM}{appearance}}}{panel}}}"#)
}

fn stat(name: &str, formula: &str, extra: &str) -> String {
    format!(
        r#"{{"motivo": "Nessuno misura quanto il treno regge.", "novita": {{"tipo": "statistica", "nome": "{name}", "descrizione": "Quanto regge il treno oggi.", "unita": "%", "scala": [0, 100], "formula": {formula}{extra}}}}}"#
    )
}

const MORALE: &str = r#"{"media": [{"peso": 100, "sorgente": {"bisogno": "sazieta"}}, {"peso": 100, "sorgente": {"bisogno": "socialita"}}, {"peso": 100, "sorgente": {"bisogno": "energia"}}]}"#;

#[test]
fn a_full_panel_passes_and_may_name_the_novelty_itself() {
    let d = ok(&item_with(
        r#", "aspetto": {"forma": "bottiglia", "colore": "grigio", "dettaglio": "etichetta"}"#,
        r#", "interfaccia": {"titolo": "Acqua in coda", "elementi": [
            {"tipo": "testo", "testo": "La latta viene dalle Officine."},
            {"tipo": "valore", "etichetta": "Borracce", "sorgente": {"scorta": "Borraccia"}, "formato": "numero"},
            {"tipo": "valore", "etichetta": "Prezzo", "sorgente": {"prezzo": "metallo", "mercato": "vicino"}, "formato": "gettoni"},
            {"tipo": "barra", "etichetta": "Sete", "sorgente": {"bisogno": "sazieta", "carrozza": "Dormitorio"}, "min": 0, "max": 1},
            {"tipo": "lista", "etichetta": "Chi le fa", "sorgente_lista": {"lavoratori": "operaio"}},
            {"tipo": "pulsante", "etichetta": "Fanne una", "azione": {"apri_crafting": "Borraccia"}},
            {"tipo": "pulsante", "etichetta": "Dall'operaio", "azione": {"parla_con_lavoro": "operaio"}},
            {"tipo": "pulsante", "etichetta": "Officina", "azione": {"vai_a": "Officina"}}]}"#,
    ));
    let panel = d.panel.as_ref().expect("a panel");
    assert_eq!(panel.elements.len(), 8);
    let Proposal::NewItem { appearance, .. } = &d.proposal else {
        panic!()
    };
    assert_eq!(appearance.as_ref().unwrap().shape, "bottiglia");
}

#[test]
fn bad_panels_are_rejected_with_the_whitelists() {
    let unknown_source = why(&item_with(
        "",
        r#", "interfaccia": {"titolo": "Acqua", "elementi": [{"tipo": "valore", "etichetta": "Sete", "sorgente": {"sete": "treno"}}]}"#,
    ));
    assert!(
        unknown_source.contains("sorgente non valida"),
        "{unknown_source}"
    );
    assert!(unknown_source.contains("riempimento"), "{unknown_source}");

    let element = r#"{"tipo": "testo", "testo": "riga"}"#;
    let nine = [element; 9].join(", ");
    let too_many = why(&item_with(
        "",
        &format!(r#", "interfaccia": {{"titolo": "Acqua", "elementi": [{nine}]}}"#),
    ));
    assert!(
        too_many.contains("da 1 a 8 elementi (ne ha 9)"),
        "{too_many}"
    );

    let unknown_item = why(&item_with(
        "",
        r#", "interfaccia": {"titolo": "Acqua", "elementi": [{"tipo": "valore", "etichetta": "Oro", "sorgente": {"scorta": "oro"}}]}"#,
    ));
    assert!(
        unknown_item.contains("l'oggetto «oro» non esiste"),
        "{unknown_item}"
    );

    let bad_action = why(&item_with(
        "",
        r#", "interfaccia": {"titolo": "Acqua", "elementi": [{"tipo": "pulsante", "etichetta": "Vai", "azione": {"teletrasporta": "Serra"}}]}"#,
    ));
    assert!(
        bad_action.contains("apri_crafting") && bad_action.contains("vai_a"),
        "{bad_action}"
    );

    let bad_kind = why(&item_with(
        "",
        r#", "interfaccia": {"titolo": "Acqua", "elementi": [{"tipo": "slider", "etichetta": "x"}]}"#,
    ));
    assert!(bad_kind.contains("pulsante"), "{bad_kind}");

    let bad_bar = why(&item_with(
        "",
        r#", "interfaccia": {"titolo": "Acqua", "elementi": [{"tipo": "barra", "etichetta": "x", "sorgente": {"giocatore": "gettoni"}, "min": 5, "max": 5}]}"#,
    ));
    assert!(bad_bar.contains("minimo"), "{bad_bar}");
}

#[test]
fn bad_appearances_list_the_allowed_values() {
    let colour = why(&item_with(
        r#", "aspetto": {"forma": "bottiglia", "colore": "fucsia"}"#,
        "",
    ));
    assert!(colour.contains("il colore «fucsia» non esiste"), "{colour}");
    assert!(colour.contains("azzurro"), "{colour}");
    let shape = why(&item_with(
        r#", "aspetto": {"forma": "sfera", "colore": "rosso"}"#,
        "",
    ));
    assert!(shape.contains("lingotto"), "{shape}");
    let detail = why(&item_with(
        r#", "aspetto": {"forma": "sacco", "colore": "rosso", "dettaglio": "fiamme"}"#,
        "",
    ));
    assert!(detail.contains("toppa"), "{detail}");
    // An appearance written next to the novelty is put back in it.
    let moved = ok(&format!(
        r#"{{"motivo": "Si beve poco e male in coda.", {ITEM}}}, "aspetto": {{"forma": "sacco", "colore": "rosso"}}}}"#
    ));
    assert!(matches!(
        moved.proposal,
        Proposal::NewItem {
            appearance: Some(_),
            ..
        }
    ));
}

#[test]
fn statistics_are_validated() {
    let d = ok(&stat(
        "Morale",
        MORALE,
        r#", "soglie": [{"sotto": 30, "testo": "Il treno è allo stremo"}]"#,
    ));
    assert_eq!(d.proposal.kind_name(), "statistica");

    let self_ref = why(&stat(
        "Morale",
        r#"{"sorgente": {"statistica": "Morale"}}"#,
        "",
    ));
    assert!(self_ref.contains("sé stessa"), "{self_ref}");

    // A cycle through an existing statistic.
    let mut k = known();
    let a: Draft = parse(&stat(
        "Rabbia",
        r#"{"sorgente": {"statistica": "Paura"}}"#,
        "",
    ))
    .unwrap();
    k.add(&a.proposal);
    let b: Draft = parse(&stat(
        "Paura",
        r#"{"sorgente": {"statistica": "Rabbia"}}"#,
        "",
    ))
    .unwrap();
    let cycle = precheck(&b, &k, MAX_EFFECTS).unwrap_err().0;
    assert!(cycle.contains("ciclo"), "{cycle}");
    assert!(cycle.contains("paura → rabbia → paura"), "{cycle}");

    let seven = [r#"{"peso": 1, "sorgente": {"giocatore": "gettoni"}}"#; 7].join(", ");
    let many = why(&stat(
        "Ricchezza",
        &format!(r#"{{"somma": [{seven}]}}"#),
        "",
    ));
    assert!(many.contains("da 1 a 6 termini"), "{many}");
    let weight = why(&stat(
        "Ricchezza",
        r#"{"somma": [{"peso": 5000, "sorgente": {"giocatore": "gettoni"}}]}"#,
        "",
    ));
    assert!(weight.contains("peso"), "{weight}");
    let unknown = why(&stat(
        "Ricchezza",
        r#"{"sorgente": {"statistica": "Nebbia"}}"#,
        "",
    ));
    assert!(
        unknown.contains("la statistica «Nebbia» non esiste"),
        "{unknown}"
    );
    let threshold = why(&stat(
        "Ricchezza",
        r#"{"sorgente": {"giocatore": "gettoni"}}"#,
        r#", "soglie": [{"testo": "boh"}]"#,
    ));
    assert!(threshold.contains("sotto"), "{threshold}");
    let scale = why(
        &stat("Ricchezza", r#"{"sorgente": {"giocatore": "gettoni"}}"#, "")
            .replace("[0, 100]", "[100, 0]"),
    );
    assert!(scale.contains("scala"), "{scale}");
}

fn world() -> World {
    let mut world = World::generate(21, 10, 80);
    let mut brain = UtilityBrain::new(21);
    world.run(&mut brain, 10 * 60);
    world
}

fn src(json: &str) -> Source {
    serde_json::from_str(json).unwrap()
}

#[test]
fn sources_read_the_world() {
    let world = world();
    let total: f32 = world
        .carriages
        .iter()
        .map(|c| c.stock.get(ItemKind::Verdura))
        .sum();
    assert_eq!(src(r#"{"scorta": "verdura"}"#).eval(&world), total.floor());
    let in_serre: f32 = world
        .carriages
        .iter()
        .filter(|c| c.kind == CarriageKind::Serra)
        .map(|c| c.stock.get(ItemKind::Verdura))
        .sum();
    assert_eq!(
        src(r#"{"scorta": "verdure", "carrozza": "Serre"}"#).eval(&world),
        in_serre.floor()
    );
    let fill = src(r#"{"riempimento": "razione"}"#).eval(&world);
    assert!((0.0..=1.0).contains(&fill), "{fill}");
    let cooks = world
        .npcs
        .iter()
        .filter(|n| n.job == Some(Job::Cuoco))
        .count();
    assert_eq!(src(r#"{"lavoratori": "cuoco"}"#).eval(&world), cooks as f32);
    assert_eq!(
        src(r#"{"popolazione": "tutti"}"#).eval(&world),
        world.npcs.len() as f32
    );
    let mean = world.npcs.iter().map(|n| n.needs.hunger).sum::<f32>() / world.npcs.len() as f32;
    assert!((src(r#"{"bisogno": "sazieta"}"#).eval(&world) - mean).abs() < 1e-5);
    assert_eq!(
        src(r#"{"giocatore": "gettoni"}"#).eval(&world),
        world.player.tokens as f32
    );
    assert_eq!(src(r#"{"giocatore": "coperta"}"#).eval(&world), 0.0);
    assert_eq!(
        src(r#"{"tempo": "giorno"}"#).eval(&world),
        world.clock.day() as f32
    );
    assert_eq!(src(r#"{"tempo": "ora"}"#).eval(&world), 16.0);
    // The cheapest Mercato price is the lowest quote.
    let cheapest = world
        .markets()
        .into_iter()
        .filter_map(|m| {
            world
                .market_quotes(m)
                .into_iter()
                .find(|q| q.item == ItemKind::Attrezzo)
        })
        .map(|q| q.price)
        .min()
        .unwrap();
    assert_eq!(
        src(r#"{"prezzo": "attrezzo"}"#).eval(&world),
        cheapest as f32
    );
    // A name that doesn't exist yet waits for the Custode.
    let pending = src(r#"{"scorta": "Borraccia"}"#).read(&world, None);
    assert!(matches!(&pending, Reading::Pending(what) if what.contains("Borraccia")));
    assert!(pending.why().unwrap().contains("in attesa del Custode"));
    assert!(src(r#"{"scorta": "Borraccia"}"#).eval(&world).is_nan());
    // Reading never changes the world.
    let before = postcard_like(&world);
    for key in SOURCE_KEYS {
        let _ = src(&format!(r#"{{"{key}": "verdura"}}"#)).read(&world, None);
    }
    assert_eq!(postcard_like(&world), before);
}

/// A cheap fingerprint of the parts sources read.
fn postcard_like(world: &World) -> String {
    format!(
        "{:?}{:?}{}",
        world.carriages.iter().map(|c| c.stock).collect::<Vec<_>>(),
        world
            .npcs
            .iter()
            .map(|n| (n.needs.hunger, n.job))
            .collect::<Vec<_>>(),
        world.player.tokens
    )
}

#[test]
fn formulas_evaluate_and_clamp() {
    let world = world();
    let morale: Statistic = match parse(&stat("Morale", MORALE, "")).unwrap().proposal {
        Proposal::Statistic(s) => s,
        _ => unreachable!(),
    };
    let n = world.npcs.len() as f32;
    let mean = |f: fn(&sim::Npc) -> f32| world.npcs.iter().map(f).sum::<f32>() / n;
    let expected = 100.0
        * (mean(|n| n.needs.hunger) + mean(|n| n.needs.social) + mean(|n| n.needs.energy))
        / 3.0;
    let got = morale.read(&world, None).value().unwrap();
    assert!((got - expected).abs() < 1e-3, "{got} vs {expected}");

    // Clamped to the scale.
    let rich: Statistic = match parse(&stat(
        "Ricchezza",
        r#"{"somma": [{"peso": 1000, "sorgente": {"popolazione": "tutti"}}]}"#,
        "",
    ))
    .unwrap()
    .proposal
    {
        Proposal::Statistic(s) => s,
        _ => unreachable!(),
    };
    assert_eq!(rich.read(&world, None), Reading::Value(100.0));
    let mut low = rich.clone();
    low.formula = serde_json::from_str(r#"{"minimo": [{"peso": -5, "sorgente": {"popolazione": "tutti"}}, {"peso": 1, "sorgente": {"tempo": "giorno"}}]}"#).unwrap();
    assert_eq!(low.read(&world, None), Reading::Value(0.0));

    // Statistics reading statistics, through the book.
    let mut book = StatBook::new();
    book.add(morale.clone());
    let mut half = rich.clone();
    half.name = "Mezzo morale".into();
    half.formula =
        serde_json::from_str(r#"{"sorgente": {"statistica": "morale"}, "peso": 0.5}"#).unwrap();
    book.add(half);
    let h = book.read("Mezzo Morale", &world).value().unwrap();
    assert!((h - expected / 2.0).abs() < 1e-3);
    assert!(matches!(book.read("Nebbia", &world), Reading::Pending(_)));
    assert_eq!(book.iter().count(), 2);
}

#[test]
fn lists_read_the_world() {
    let world = world();
    let list: narrator::ListSource = serde_json::from_str(r#"{"lavoratori": "cuoco"}"#).unwrap();
    let rows = list.rows(&world, 8).unwrap();
    let cooks = world
        .npcs
        .iter()
        .filter(|n| n.job == Some(Job::Cuoco))
        .count();
    assert_eq!(rows.rows.len() + rows.more, cooks);
    assert!(rows.rows.len() <= 8);
    let carriages: narrator::ListSource =
        serde_json::from_str(r#"{"carrozze_con": "verdura"}"#).unwrap();
    let rows = carriages.rows(&world, 3).unwrap().rows;
    let values: Vec<u32> = rows.iter().map(|r| r.value.parse().unwrap()).collect();
    assert!(values.windows(2).all(|w| w[0] >= w[1]), "{values:?}");
    let pending: narrator::ListSource = serde_json::from_str(r#"{"prezzi": "Borraccia"}"#).unwrap();
    assert!(matches!(pending.rows(&world, 8), Err(Reading::Pending(_))));
}

#[test]
fn the_prompt_lists_the_whitelists() {
    let p = system_prompt(3);
    for word in SHAPES.iter().chain(&COLOURS).chain(&DETAILS) {
        assert!(p.contains(word), "no {word:?}");
    }
    for key in SOURCE_KEYS.iter().chain(&LIST_KEYS).chain(&ACTION_KEYS) {
        assert!(p.contains(&format!("\"{key}\"")), "no {key:?}");
    }
    for kind in ELEMENT_KINDS {
        assert!(
            p.contains(&format!("\"tipo\":\"{kind}\"")),
            "no element {kind:?}"
        );
    }
    for part in [
        "\"interfaccia\"",
        "\"statistica\"",
        "\"aspetto\"",
        "\"media\"",
        "\"soglie\"",
    ] {
        assert!(p.contains(part), "no {part}");
    }
    // One example each, and the examples pass the guard.
    let example = p
        .split("Esempio di risposta:\n")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    let d = parse(example).unwrap();
    assert!(d.panel.is_some());
    let stat_example = p
        .split("Esempio di statistica (la \"novita\"):\n")
        .nth(1)
        .unwrap();
    let s = format!(
        r#"{{"motivo": "Nessuno misura quanto il treno regge.", "novita": {stat_example}}}"#
    );
    assert!(matches!(ok(&s).proposal, Proposal::Statistic(_)));
}

#[test]
fn a_mock_round_trip_with_a_panel_and_a_statistic() {
    let answers = [
        item_with(
            r#", "aspetto": {"forma": "bottiglia", "colore": "grigio"}"#,
            r#", "interfaccia": {"titolo": "Acqua in coda", "elementi": [{"tipo": "valore", "etichetta": "Borracce", "sorgente": {"scorta": "Borraccia"}}, {"tipo": "pulsante", "etichetta": "Fanne una", "azione": {"apri_crafting": "Borraccia"}}]}"#,
        ),
        {
            let s = stat(
                "Morale",
                MORALE,
                r#", "soglie": [{"sotto": 30, "testo": "Il treno è allo stremo"}]"#,
            );
            format!(
                r#"{}, "interfaccia": {{"titolo": "Morale", "elementi": [{{"tipo": "pulsante", "etichetta": "Dettagli", "azione": {{"mostra_statistica": "Morale"}}}}]}}}}"#,
                &s[..s.len() - 1]
            )
        },
    ];
    let llm = Arc::new(MockLlm::scripted(answers.iter().map(|a| Ok(a.clone()))));
    let mut n = Narrator::new(NarratorConfig::new(llm, Budget::unlimited()));
    let world = world();
    let summary = WorldSummary::from_world(&world);
    let mut drafts = Vec::new();
    for day in [1, 2] {
        assert_eq!(n.request(&summary, day), Requested::Sent);
        let o = n.wait(Duration::from_secs(5)).unwrap();
        let Verdict::Accepted(d) = o.verdict else {
            panic!("{o:?}")
        };
        drafts.push(d);
    }
    assert!(drafts[0].panel.is_some());
    assert!(matches!(drafts[1].proposal, Proposal::Statistic(_)));
    assert_eq!(drafts[1].panel.as_ref().unwrap().elements.len(), 1);
    // Through JSON, as in a save.
    let json = serde_json::to_string(&drafts).unwrap();
    let back: Vec<Draft> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, drafts);
    // Restored after a load: the names stay taken.
    n.restore(back.into_iter().map(|d| (1, d)), Some(2));
    assert_eq!(n.novelties().len(), 2);
    assert_eq!(n.request(&summary, 2), Requested::AlreadyToday);
}
