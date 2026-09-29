//! Tests of the conversation text: bubbles, arcs, grammar.

use std::collections::HashSet;

use super::*;
use crate::names;

fn voice(id: u32, first: &'static str, sex: Sex, job: Option<Job>) -> Voice<'static> {
    Voice {
        id: NpcId(id),
        first,
        sex,
        age: 35,
        job,
        personality: Personality::default(),
    }
}

fn subject(sex: Sex) -> Subject {
    Subject {
        first: sex.pick("Marta", "Luca").to_string(),
        surname: "Rossi".to_string(),
        sex,
        other: sex.pick("Gino", "Anna").to_string(),
    }
}

fn script<'a>(
    topic: Topic,
    tone: Tone,
    news: Option<News>,
    about: Option<&'a Subject>,
) -> Script<'a> {
    Script {
        topic,
        tone,
        news,
        a: voice(1, "Marta", Sex::Female, Some(Job::Cuoco)),
        b: voice(2, "Luca", Sex::Male, Some(Job::Operaio)),
        tie: None,
        about,
        fond: true,
        need: Need::Hunger,
        place: "Il Refettorio",
        hour: 10,
    }
}

const NEWS: [News; 24] = [
    News::Fight,
    News::Killing,
    News::Birth,
    News::Death,
    News::Couple,
    News::Widowed,
    News::TheftCaught,
    News::TheftUnseen,
    News::HelpGiven,
    News::HelpRefused,
    News::Protest(Grievance::BirthDenied),
    News::Protest(Grievance::FoodShortage),
    News::Concession(Grievance::BirthDenied),
    News::Concession(Grievance::FoodShortage),
    News::Shortage(ItemKind::Razione),
    News::Shortage(ItemKind::Vestito),
    News::Restocked(ItemKind::Attrezzo),
    News::BirthDenied,
    News::CameOfAge,
    News::Retired,
    News::Austerity,
    News::PayRaised,
    News::PayCut,
    News::Restocked(ItemKind::Razione),
];

/// Every carriage name the generator can give (named ones and a few
/// numbered ones of each kind).
fn carriage_names() -> Vec<String> {
    let mut all: Vec<String> = [
        names::DORM_NAMES,
        names::MENSA_NAMES,
        names::SERRA_NAMES,
        names::OFFICINA_NAMES,
        names::MERCATO_NAMES,
    ]
    .iter()
    .flat_map(|l| l.iter().map(|n| n.to_string()))
    .collect();
    for kind in crate::CarriageKind::ALL {
        for n in [4, 5, 12] {
            all.push(format!("{} {n}", kind.name()));
        }
    }
    all
}

/// No combination of personality tags: each alone, and a few pairs.
fn personalities() -> Vec<Personality> {
    let mut all = vec![Personality::default()];
    for t in Temper::ALL {
        all.push(Personality::default().with(t));
    }
    all.push(
        Personality::default()
            .with(Temper::Curioso)
            .with(Temper::Lamentoso),
    );
    all.push(
        Personality::default()
            .with(Temper::Pettegolo)
            .with(Temper::Timido),
    );
    all
}

/// Templates only found in `pools` (not in any other pool).
fn only_in(pools: &[Pool]) -> HashSet<&'static str> {
    let inside: HashSet<&'static str> = pools.iter().flat_map(|p| p.iter().copied()).collect();
    let outside: HashSet<&'static str> = ALL_POOLS
        .iter()
        .filter(|p| !pools.contains(p))
        .flat_map(|p| p.iter().copied())
        .collect();
    inside.difference(&outside).copied().collect()
}

fn union(pools: &[Pool]) -> HashSet<&'static str> {
    pools.iter().flat_map(|p| p.iter().copied()).collect()
}

/// Lines of sad and of happy pools.
fn sad_pools() -> Vec<Pool> {
    vec![
        BAD_REPLY_FRIENDLY,
        BAD_REPLY_NEUTRAL,
        BAD_REPLY_TENSE,
        BAD_FOLLOW,
        LAMENT_BAD,
        LAMENT_GRIPE,
        SHY_BAD,
        SHY_GRIPE,
        CURT_BAD,
        CURT_GRIPE,
        KIND_BAD,
        KIND_GRIPE,
        CHEERFUL_GRIPE,
        REACT_BAD_FRIENDLY,
        REACT_BAD_NEUTRAL,
        REACT_GRIPE_FRIENDLY,
        REACT_GRIPE_NEUTRAL,
        REACT_GRIPE_TENSE,
        CLOSE_SOMBER,
        KID_BAD,
        INSIST_BAD,
        DEATH_REPLY,
        DEATH_FOLLOW,
        WIDOWED_REPLY,
        WIDOWED_FOLLOW,
        COMPLAINT_REPLY_FRIENDLY,
        COMPLAINT_REPLY_NEUTRAL,
        COMPLAINT_REPLY_TENSE,
        COMPLAINT_FOOD_FOLLOW,
        COMPLAINT_MONEY_FOLLOW,
    ]
}

fn happy_pools() -> Vec<Pool> {
    vec![
        GOOD_REPLY_FRIENDLY,
        GOOD_REPLY_NEUTRAL,
        GOOD_REPLY_TENSE,
        GOOD_FOLLOW,
        LAMENT_GOOD,
        SHY_GOOD,
        CURT_GOOD,
        CHEERFUL_GOOD,
        REACT_GOOD_FRIENDLY,
        REACT_GOOD_NEUTRAL,
        KID_GOOD,
        INSIST_GOOD,
        BIRTH_REPLY,
        BIRTH_FOLLOW,
        COUPLE_REPLY,
        COUPLE_FOLLOW,
        RETIRED_REPLY,
        PAY_RAISED_FOLLOW,
        CONCESSION_FOOD_FOLLOW,
    ]
}

/// The answers allowed for a piece of news by its valence and the tone.
fn allowed_replies(news: News, tone: Tone) -> HashSet<&'static str> {
    let mood = mood_of(news.valence());
    let thread = news_thread(news, true, false);
    let mut pools: Vec<Pool> = vec![by_tone_of(tone, thread.reply)];
    if tone != Tone::Tense {
        pools.extend([shy_reply(mood), curt(mood), CURIOUS]);
        pools.extend(kind(mood));
        pools.extend(cheerful(
            &script(Topic::News, tone, Some(news), None),
            mood,
            false,
        ));
    }
    union(&pools)
}

fn by_tone_of(tone: Tone, pools: [Pool; 3]) -> Pool {
    pools[tone.index()]
}

#[test]
fn every_template_fits_a_bubble_with_ordinary_names() {
    assert!(template_count() >= 350, "{} templates", template_count());
    for sex in Sex::ALL {
        let about = subject(sex.opposite());
        for pool in ALL_POOLS {
            for template in pool.iter() {
                for job in [None, Some(Job::Operaio)] {
                    let mut s = script(
                        Topic::News,
                        Tone::Neutral,
                        Some(News::Shortage(ItemKind::Attrezzo)),
                        Some(&about),
                    );
                    s.a = voice(1, "Marta", sex, job);
                    s.b = voice(2, "Luca", sex.opposite(), job);
                    for side in [Side::A, Side::B] {
                        let text = render(template, &s, side).expect(template);
                        assert!(
                            text.chars().count() <= MAX_LINE_CHARS,
                            "{text:?} ({} chars)",
                            text.chars().count()
                        );
                        assert!(!text.contains('{') && !text.contains('}'), "{text}");
                    }
                }
            }
        }
    }
}

/// With any carriage name, every pool keeps at least one line that fits.
#[test]
fn every_pool_fits_in_every_carriage() {
    let about = subject(Sex::Female);
    for place in carriage_names() {
        let mut s = script(Topic::News, Tone::Neutral, None, Some(&about));
        s.place = &place;
        for pool in ALL_POOLS {
            let fits = pool.iter().any(|t| {
                render(t, &s, Side::A).is_some_and(|x| x.chars().count() <= MAX_LINE_CHARS)
            });
            assert!(fits, "{place}: {pool:?}");
        }
    }
}

#[test]
fn every_topic_and_tone_writes_lines_for_both_sexes() {
    let long = Subject {
        first: "Margherita".to_string(),
        surname: "De Santis".to_string(),
        sex: Sex::Female,
        other: "Massimiliano".to_string(),
    };
    let mut seed = 0;
    for topic in Topic::ALL {
        for tone in Tone::ALL {
            for sex in Sex::ALL {
                for news in NEWS.iter().map(|&n| Some(n)).chain([None]) {
                    for tie in [
                        None,
                        Some(RelationKind::Partner),
                        Some(RelationKind::Parent),
                        Some(RelationKind::Child),
                        Some(RelationKind::Sibling),
                    ] {
                        for about in [None, Some(&long)] {
                            seed += 1;
                            let mut s = script(topic, tone, news, about);
                            s.a = voice(1, "Alessandro", sex, Some(Job::Mercante));
                            s.b = voice(2, "Margherita", sex.opposite(), None);
                            s.tie = tie;
                            s.fond = seed % 2 == 0;
                            s.need = Need::Tiredness;
                            s.place = "Giardino d'Inverno";
                            s.hour = (seed % 24) as u32;
                            let (since, until) = (GameTime(1000), GameTime(1000 + seed % 50));
                            let lines = write(&s, seed, since, until);
                            assert!((3..=6).contains(&lines.len()));
                            for (k, l) in lines.iter().enumerate() {
                                assert!(!l.text.is_empty());
                                assert!(l.text.chars().count() <= MAX_LINE_CHARS, "{}", l.text);
                                assert!(!l.text.contains('{'), "{}", l.text);
                                assert!(l.at >= since && l.at <= until.max(since + 1));
                                let speaker = if k % 2 == 0 { NpcId(1) } else { NpcId(2) };
                                assert_eq!(l.speaker, speaker);
                            }
                            assert_eq!(lines, write(&s, seed, since, until), "deterministic");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn gender_and_personality_shape_the_lines() {
    let s = |sex| {
        let mut s = script(Topic::Needs, Tone::Friendly, None, None);
        s.a = voice(1, "Marta", sex, None);
        s.b = voice(2, "Luca", Sex::Male, None);
        s.need = Need::Tiredness;
        s.place = "Nido";
        s
    };
    let t = "Sono stanc{S:a/o} mort{S:a/o}, {n}.";
    assert_eq!(
        render(t, &s(Sex::Female), Side::A).unwrap(),
        "Sono stanca morta, Luca."
    );
    assert_eq!(
        render(t, &s(Sex::Male), Side::A).unwrap(),
        "Sono stanco morto, Luca."
    );
    let mut worker = s(Sex::Female);
    worker.a.job = Some(Job::Operaio);
    assert_eq!(
        render("Fare {k}.", &worker, Side::A).unwrap(),
        "Fare l'operaia."
    );
    worker.a.job = Some(Job::Cuoco);
    assert_eq!(
        render("Fare {k}.", &worker, Side::A).unwrap(),
        "Fare la cuoca."
    );
    worker.a.job = Some(Job::Mercante);
    assert_eq!(
        render("Fare {k}.", &worker, Side::A).unwrap(),
        "Fare la mercante."
    );
    // Two shy people: some lines are just "…" or a stammer.
    let mut shy = s(Sex::Female);
    shy.a.personality = Personality::default().with(Temper::Timido);
    shy.b.personality = shy.a.personality;
    let shy_lines = union(&[
        SHY, SHY_BAD, SHY_GRIPE, SHY_GOOD, SHY_FOLLOW, SHY_CLOSE, SHY_GREET,
    ]);
    let silent = (0..200)
        .flat_map(|seed| compose(&shy, seed, GameTime(0), GameTime(48)))
        .filter(|(_, t, _)| shy_lines.contains(t))
        .count();
    assert!(silent > 20, "{silent}");
    // Babies babble.
    let mut baby = s(Sex::Male);
    baby.b.age = 1;
    let lines = write(&baby, 3, GameTime(0), GameTime(30));
    assert!(BABBLE.contains(&lines[1].text.as_str()));
    // Items agree in gender with their article.
    let shortage = |item| {
        let mut s = script(
            Topic::News,
            Tone::Friendly,
            Some(News::Shortage(item)),
            None,
        );
        s.news = Some(News::Restocked(item));
        render("Sono tornat{I:e/i} {li}!", &s, Side::A).unwrap()
    };
    assert_eq!(shortage(ItemKind::Razione), "Sono tornate le razioni!");
    assert_eq!(shortage(ItemKind::Attrezzo), "Sono tornati gli attrezzi!");
    assert_eq!(shortage(ItemKind::Vestito), "Sono tornati i vestiti!");
}

#[test]
fn placeholders_join_with_proper_grammar() {
    let couple = Subject {
        first: "Bruno".to_string(),
        surname: "Esposito".to_string(),
        sex: Sex::Male,
        other: "Elena".to_string(),
    };
    let mut s = script(
        Topic::News,
        Tone::Friendly,
        Some(News::Couple),
        Some(&couple),
    );
    s.place = "Alveare";
    let r = |t: &str, s: &Script| render(t, s, Side::A).unwrap();
    assert_eq!(
        r("{a} e {o} stanno insieme!", &s),
        "Bruno ed Elena stanno insieme!"
    );
    assert_eq!(
        r("Tra {a} e {o} c'è del tenero!", &s),
        "Tra Bruno ed Elena c'è del tenero!"
    );
    assert_eq!(
        r("Lo dicono tutti {al}.", &s),
        "Lo dicono tutti all'Alveare."
    );
    assert_eq!(r("{al} c'è gente.", &s), "All'Alveare c'è gente.");
    assert_eq!(r("{nel} fa freddo.", &s), "Nell'Alveare fa freddo.");
    assert_eq!(r("Auguri {ai}!", &s), "Auguri agli Esposito!");
    assert_eq!(r("{fam} sono contenti.", &s), "Gli Esposito sono contenti.");
    assert_eq!(r("È nata {a}, {dei}!", &s), "È nata Bruno, degli Esposito!");
    s.b.first = "Anna";
    assert_eq!(r("Dillo a {n}.", &s), "Dillo ad Anna.");
    s.place = "La Brace";
    assert_eq!(
        r("Lo dicono tutti {al}.", &s),
        "Lo dicono tutti alla Brace."
    );
    s.place = "Dormitorio 6";
    assert_eq!(r("Ne parlano {nel}.", &s), "Ne parlano nel Dormitorio 6.");
    // Missing data: the template is skipped.
    let s = script(Topic::News, Tone::Friendly, Some(News::Couple), None);
    assert_eq!(render("{a} e {o} stanno insieme!", &s, Side::A), None);
    // Greetings by the hour.
    let mut s = script(Topic::Family, Tone::Friendly, None, None);
    s.hour = 8;
    assert_eq!(
        r("{g}, {n}! {bg}!", &s),
        "Buongiorno, Luca! Buona giornata!"
    );
    s.hour = 20;
    assert_eq!(r("{g}, {n}! {bg}!", &s), "Buonasera, Luca! Buona serata!");
    s.hour = 23;
    assert_eq!(r("{bg}!", &s), "Buonanotte!");
}

/// Across every topic, tone, news, personality and carriage: no grammar
/// slips ("a Alveare", "Bruno e Elena", double spaces, lowercase starts).
#[test]
fn lines_have_no_grammar_slips() {
    let places = carriage_names();
    let place_refs: Vec<&str> = places.iter().map(String::as_str).collect();
    let couple = Subject {
        first: "Andrea".to_string(),
        surname: "Esposito".to_string(),
        sex: Sex::Female,
        other: "Elena".to_string(),
    };
    let people = personalities();
    let mut seed = 0u64;
    for place in &places {
        for topic in Topic::ALL {
            for news in NEWS.iter().map(|&n| Some(n)).chain([None]) {
                for tone in Tone::ALL {
                    seed += 1;
                    let mut s = script(topic, tone, news, Some(&couple));
                    s.place = place;
                    s.a.personality = people[(seed % 11) as usize];
                    s.b.first = "Anna";
                    s.b.personality = people[(seed / 11 % 11) as usize];
                    s.tie = [None, Some(RelationKind::Partner)][(seed % 2) as usize];
                    for (_, _, l) in compose(&s, seed, GameTime(0), GameTime(60)) {
                        let slips = grammar::slips(&l.text, &place_refs);
                        assert!(slips.is_empty(), "{:?}: {slips:?}", l.text);
                    }
                }
            }
        }
    }
}

/// The lines of every conversation about a piece of news: (beat, template).
fn news_arcs(news: News, tone: Tone) -> Vec<Vec<(Beat, &'static str)>> {
    let about = subject(Sex::Female);
    let mut arcs = Vec::new();
    let mut seed = 0;
    for topic in [Topic::News, Topic::Gossip] {
        for pa in personalities() {
            for pb in personalities() {
                for _ in 0..3 {
                    seed += 1;
                    let mut s = script(topic, tone, Some(news), Some(&about));
                    s.a.personality = pa;
                    s.b.personality = pb;
                    let lines = compose(&s, seed, GameTime(0), GameTime(20 + seed % 40));
                    arcs.push(lines.into_iter().map(|(b, t, _)| (b, t)).collect());
                }
            }
        }
    }
    arcs
}

#[test]
fn answers_and_follow_ups_match_the_news() {
    let sad = only_in(&sad_pools());
    let happy = only_in(&happy_pools());
    assert!(sad.contains("Sempre peggio, sempre peggio…"));
    assert!(happy.contains("Che bella notizia!"));
    for news in NEWS {
        let valence = news.valence();
        let mood = mood_of(valence);
        let (_, specific) = news_specific(news);
        for tone in Tone::ALL {
            let replies = allowed_replies(news, tone);
            let mut follows = union(&[
                specific,
                generic_follow(mood),
                answer(mood),
                insist(mood),
                lament(mood),
                SECRET,
                SHY_FOLLOW,
            ]);
            if news == News::Couple || news == News::Birth || news == News::Widowed {
                // Gossip about them opens with the gossip pool, same follow-ups.
                follows.extend(news_specific(news).1.iter().copied());
            }
            for arc in news_arcs(news, tone) {
                for &(beat, template) in &arc {
                    match valence {
                        Valence::Good => assert!(!sad.contains(template), "{news:?}: {arc:?}"),
                        Valence::Bad => assert!(!happy.contains(template), "{news:?}: {arc:?}"),
                        Valence::Scandal => {
                            assert!(!happy.contains(template), "{news:?}: {arc:?}")
                        }
                    }
                    match beat {
                        Beat::Reply => {
                            assert!(replies.contains(template), "{news:?} {tone:?}: {arc:?}")
                        }
                        Beat::Follow => {
                            assert!(follows.contains(template), "{news:?} {tone:?}: {arc:?}");
                            if tone == Tone::Tense {
                                assert!(insist(mood).contains(&template), "{arc:?}");
                            }
                        }
                        _ => {}
                    }
                }
                // A question gets an answer.
                if let Some(k) = arc
                    .iter()
                    .position(|(b, t)| *b == Beat::Reply && CURIOUS.contains(t))
                {
                    assert_eq!(arc[k + 1].0, Beat::Follow, "{arc:?}");
                    assert!(answer(mood).contains(&arc[k + 1].1), "{arc:?}");
                }
            }
        }
    }
}

/// The reported case: good news, a curious listener, a complaining teller.
#[test]
fn good_news_never_turns_into_a_lament() {
    let couple = Subject {
        first: "Bruno".to_string(),
        surname: "Rossi".to_string(),
        sex: Sex::Male,
        other: "Elena".to_string(),
    };
    let mut s = script(
        Topic::News,
        Tone::Friendly,
        Some(News::Couple),
        Some(&couple),
    );
    s.a.personality = Personality::default().with(Temper::Lamentoso);
    s.b.personality = Personality::default().with(Temper::Curioso);
    let mut asked = 0;
    for seed in 0..500 {
        let lines = compose(&s, seed, GameTime(0), GameTime(48));
        assert!(lines[0].2.text.contains("Bruno") && lines[0].2.text.contains("Elena"));
        for (_, t, l) in &lines {
            assert!(
                !LAMENT_BAD.contains(t) && !BAD_FOLLOW.contains(t),
                "{}",
                l.text
            );
        }
        if CURIOUS.contains(&lines[1].1) {
            asked += 1;
            assert!(ANSWER.contains(&lines[2].1), "{lines:?}");
        }
    }
    assert!(asked > 50, "{asked}");
}

#[test]
fn complaints_are_agreed_with_or_pushed_back() {
    for news in [
        None,
        Some(News::Shortage(ItemKind::Razione)),
        Some(News::BirthDenied),
        Some(News::PayCut),
    ] {
        for tone in Tone::ALL {
            let s = script(Topic::Complaint, tone, news, None);
            let agree = union(&[
                COMPLAINT_REPLY_FRIENDLY,
                COMPLAINT_FOOD_FRIENDLY,
                COMPLAINT_BIRTH_FRIENDLY,
                COMPLAINT_MONEY_FRIENDLY,
            ]);
            let shrug = union(&[
                COMPLAINT_REPLY_NEUTRAL,
                COMPLAINT_BIRTH_NEUTRAL,
                COMPLAINT_MONEY_NEUTRAL,
            ]);
            let push = union(&[
                COMPLAINT_REPLY_TENSE,
                COMPLAINT_FOOD_TENSE,
                COMPLAINT_BIRTH_TENSE,
                COMPLAINT_MONEY_TENSE,
            ]);
            for seed in 0..60 {
                let lines = compose(&s, seed, GameTime(0), GameTime(48));
                let reply = lines.iter().find(|l| l.0 == Beat::Reply).unwrap().1;
                let expected = match tone {
                    Tone::Friendly => &agree,
                    Tone::Neutral => &shrug,
                    Tone::Tense => &push,
                };
                assert!(expected.contains(reply), "{tone:?}: {reply}");
                if let Some(f) = lines.iter().find(|l| l.0 == Beat::Follow) {
                    let ok = match tone {
                        Tone::Tense => INSIST_GRIPE.contains(&f.1),
                        _ => !union(&[INSIST_GRIPE, SMALL_REMARK, GOOD_FOLLOW]).contains(f.1),
                    };
                    assert!(ok, "{tone:?}: {}", f.1);
                }
            }
        }
    }
}

/// Every thread's follow-up stays with its opener: a conversation about
/// work never drifts to the weather, one about the family never to news.
#[test]
fn follow_ups_stay_on_the_subject() {
    let about = subject(Sex::Male);
    let off_topic = union(&[SMALL_REMARK, GOOD_FOLLOW, BAD_FOLLOW, SCANDAL_FOLLOW, KID]);
    for topic in [Topic::Work, Topic::Needs, Topic::Family, Topic::Complaint] {
        for tie in [None, Some(RelationKind::Partner), Some(RelationKind::Child)] {
            for p in personalities() {
                for seed in 0..20 {
                    let mut s = script(topic, Tone::Friendly, None, Some(&about));
                    s.tie = tie;
                    s.a.personality = p;
                    let lines = compose(&s, seed, GameTime(0), GameTime(60));
                    for (beat, t, _) in &lines {
                        if *beat == Beat::Follow {
                            assert!(!off_topic.contains(t), "{topic:?}: {lines:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn greetings_come_first_and_goodbyes_last() {
    let mut s = script(Topic::Family, Tone::Friendly, None, None);
    s.tie = Some(RelationKind::Partner);
    let mut greeted = 0;
    for seed in 0..200 {
        let lines = compose(&s, seed, GameTime(0), GameTime(48));
        let beats: Vec<Beat> = lines.iter().map(|l| l.0).collect();
        assert_eq!(beats.iter().filter(|b| **b == Beat::Statement).count(), 1);
        assert!(matches!(beats.last(), Some(Beat::Close | Beat::CloseBack)));
        if beats[0] == Beat::Greet {
            greeted += 1;
            assert_eq!(beats[1], Beat::GreetBack);
            assert_eq!(beats[2], Beat::Statement);
        }
    }
    assert!(greeted > 20, "{greeted}");
    // News opens straight away.
    let about = subject(Sex::Female);
    let s = script(Topic::News, Tone::Friendly, Some(News::Birth), Some(&about));
    for seed in 0..50 {
        assert_eq!(
            compose(&s, seed, GameTime(0), GameTime(48))[0].0,
            Beat::Statement
        );
    }
}
