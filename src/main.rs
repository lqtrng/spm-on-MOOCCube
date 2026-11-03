mod audit;
mod data;
mod export;
mod model;
mod prefixspan;

use audit::Audit;
use data::{Dataset, Event, Itemset};
use model::{Agg, Config, ConstantTopic, Evaluator, LastTopic, ProbabilityModel};
use prefixspan::{Limits, Patterns};
use std::time::Instant;

const DATA_DIR: &str = "data/mooc";

/// Picked by `tune` on the validation set.
const CHOSEN: Config =
    Config { min_support: 10, max_len: 3, agg: Agg::Sum, alpha: 0.0, len_weight: 4.0, recency: 0.0 };

/// One row per video of each session, in the order the sessions were cut.
fn itemset_sessions(events: &[Event], ds: &Dataset, name: &str, audit: &Audit) -> Vec<Vec<Itemset>> {
    let sessions = data::sessions(events);
    let itemsets: Vec<Vec<Itemset>> = sessions.iter().map(|s| data::to_itemsets(s, &ds.video_topics)).collect();
    audit.log(&format!("events_{name}"), events.len());
    audit.log(&format!("sessions_{name}"), sessions.len());
    audit.table(
        &format!("sessions_{name}.tsv"),
        "session\tuser\tposition\tts\tvideo\ttopics",
        sessions.iter().zip(&itemsets).enumerate().flat_map(|(i, (s, topics))| {
            s.iter().zip(topics).enumerate().map(move |(p, (e, ts))| {
                format!("{i}\t{}\t{p}\t{}\t{}\t{}", e.user, e.ts, e.video, audit::itemset(ts))
            })
        }),
    );
    itemsets
}

/// Mines under the default guardrails. The support actually used may exceed `min_support`.
fn mine_timed(train: &[Vec<Itemset>], min_support: u32, max_len: usize, audit: &Audit) -> (Patterns, u32) {
    let start = Instant::now();
    let (p, support) = prefixspan::mine_within_limits(train, min_support, max_len, Limits::default());
    audit.log("min_support_used", support);
    audit.log("patterns", p.nodes.len() - 1);
    audit.patterns(&p);
    println!(
        "PrefixSpan: {} sequences, min_support={support}, max_len={max_len} -> {} patterns in {:.1}s",
        train.len(),
        p.nodes.len() - 1,
        start.elapsed().as_secs_f64()
    );
    (p, support)
}

/// Mines exactly as `CHOSEN` says: a config that no longer fits the guardrails is an error here,
/// because silently raising the support would evaluate a different model.
fn mine_exact(train: &[Vec<Itemset>], audit: &Audit) -> Patterns {
    let (p, support) = mine_timed(train, CHOSEN.min_support, CHOSEN.max_len, audit);
    assert_eq!(support, CHOSEN.min_support, "CHOSEN does not fit the guardrails");
    p
}

/// Grid search. Reads train and val only: the test set stays untouched here.
fn tune(lowest_support: u32, max_len: usize) {
    let audit = Audit::build("tune");
    audit.log("lowest_support", lowest_support);
    audit.log("max_len", max_len);
    let ds = data::ETL::new(DATA_DIR.into()).run(&audit);
    let (train_events, val_events, _) = ds.split_by_user();
    let train = itemset_sessions(&train_events, &ds, "train", &audit);
    let val_sessions = itemset_sessions(&val_events, &ds, "val", &audit);
    let val = Evaluator::build(&val_sessions, max_len, "val", &audit);
    println!("val: {} pairs, {} distinct queries", val.total, val.questions.len());

    let base_views = val.evaluate_exported(&ConstantTopic::build(&train_events, &ds, &audit), "most_views", &audit);
    let base_last = val.evaluate_exported(&LastTopic::build(&train, &audit), "last_topic", &audit);
    println!(
        "baselines on val: last topic {:.2}%, most views {:.2}%",
        100.0 * base_last, 100.0 * base_views
    );

    let (patterns, lowest_support) = mine_timed(&train, lowest_support, max_len, &audit);
    let by_len: Vec<(usize, Evaluator)> = [2, 3, 5, 7]
        .into_iter()
        .filter(|l| *l <= max_len)
        .map(|l| (l, Evaluator::build(&val_sessions, l, &format!("val_len{l}"), &audit)))
        .collect();
    let mut results = Vec::new();
    for min_support in [lowest_support, 2 * lowest_support, 5 * lowest_support, 20 * lowest_support] {
        for agg in [Agg::Sum, Agg::Max] {
            for (len, val) in &by_len {
                for alpha in [0.0, 0.5] {
                    for len_weight in [2.0, 4.0] {
                        for recency in [0.5, 0.0] {
                            let cfg = Config { min_support, max_len: *len, agg, alpha, len_weight, recency };
                            let score = val.evaluate(&ProbabilityModel::build(&patterns, cfg));
                            audit.append("tune_results.tsv", "hit_rate\tconfig", format!("{score}\t{cfg:?}"));
                            results.push((score, cfg));
                        }
                    }
                }
            }
        }
    }
    results.sort_by(|a: &(f64, Config), b| b.0.total_cmp(&a.0));
    for (score, cfg) in results.iter().take(12) {
        println!("{:.3}%  {cfg:?}", 100.0 * score);
    }
    println!("--- best at each min_support, with its pattern count");
    for min_support in [lowest_support, 2 * lowest_support, 5 * lowest_support, 20 * lowest_support] {
        let n = patterns.nodes.iter().filter(|x| x.support >= min_support).count() - 1;
        if let Some((score, cfg)) = results.iter().find(|(_, c)| c.min_support == min_support && c.max_len == 7) {
            println!("{:.3}% with {n} patterns  {cfg:?}", 100.0 * score);
        }
    }
    println!("--- best at each max_len");
    for len in [2, 3, 4, 5, 7] {
        if let Some((score, cfg)) = results.iter().find(|(_, c)| c.max_len == len) {
            println!("{:.3}%  {cfg:?}", 100.0 * score);
        }
    }
    let worst = results.last().unwrap();
    println!("worst: {:.3}%  {:?}", 100.0 * worst.0, worst.1);
}

/// Mines with `CHOSEN`, scores the test set, writes `demo.html`. Run once tuning is over.
fn evaluate() {
    let audit = Audit::build("evaluate");
    audit.log("config", format!("{CHOSEN:?}"));
    let ds = data::ETL::new(DATA_DIR.into()).run(&audit);
    let (train_events, _, test_events) = ds.split_by_user();
    let train = itemset_sessions(&train_events, &ds, "train", &audit);
    let test = itemset_sessions(&test_events, &ds, "test", &audit);
    let patterns = mine_exact(&train, &audit);
    let model = ProbabilityModel::build(&patterns, CHOSEN);

    let evaluator = Evaluator::build(&test, CHOSEN.max_len, "test", &audit);
    let score = evaluator.evaluate_exported(&model, "probability", &audit);
    let base_views = evaluator.evaluate_exported(&ConstantTopic::build(&train_events, &ds, &audit), "most_views", &audit);
    let base_last = evaluator.evaluate_exported(&LastTopic::build(&train, &audit), "last_topic", &audit);

    let pct = |x: f64| format!("{:.2}%", 100.0 * x);
    println!("Test pairs (query, next video): {}", evaluator.total);
    println!("Pattern model, Hit@1: {}", pct(score));
    println!("Baseline: always the last topic: {}", pct(base_last));
    println!("Baseline: always the topic with most views: {}", pct(base_views));
    println!("config: {CHOSEN:?}");

    let demo = export::demo_html(&patterns, &CHOSEN, &ds.topic_names);
    std::fs::write("demo.html", &demo).expect("write demo.html");
    println!("demo.html: {} KB, {} patterns", demo.len() / 1024, patterns.nodes.len() - 1);
    let cases: Vec<Vec<Itemset>> = evaluator.questions.iter().map(|q| q.query.clone()).collect();
    std::fs::write("target/selftest.html", export::selftest_html(&demo, &model, &cases)).expect("write selftest");
    println!("target/selftest.html: {} distinct test queries", cases.len());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("tune") => tune(args[2].parse().unwrap(), args[3].parse().unwrap()),
        Some("evaluate") => evaluate(),
        _ => eprintln!("usage: topicrec tune <lowest_support> <max_len> | evaluate"),
    }
}
