//! Next-topic models, the probability model and the baselines, and their evaluation.

use crate::audit::{self, Audit};
use crate::data::{self, Dataset, Event, Itemset, Topic};
use crate::prefixspan::{Node, Patterns};
use rayon::prelude::*;
use std::collections::HashMap;

/// Predicts the next video's topic from a query of the session's nearest videos, oldest first.
pub trait Model: Sync {
    fn predict(&self, query: &[Itemset]) -> Topic;
}

/// Baseline: always the same topic.
pub struct ConstantTopic {
    cache: Topic,
}

impl ConstantTopic {
    /// The topic with the most views.
    pub fn build(train: &[Event], ds: &Dataset, audit: &Audit) -> Self {
        let mut views: HashMap<Topic, usize> = HashMap::new();
        for s in data::sessions(train) {
            for e in s {
                for &t in &ds.video_topics[&e.video] {
                    *views.entry(t).or_default() += 1;
                }
            }
        }
        let mut ranked: Vec<(Topic, usize)> = views.into_iter().collect();
        ranked.sort_unstable_by_key(|(t, c)| (std::cmp::Reverse(*c), *t));
        audit.table("topic_views.tsv", "topic\tviews", ranked.iter().map(|(t, c)| format!("{t}\t{c}")));
        ConstantTopic { cache: ranked[0].0 }
    }
}

impl Model for ConstantTopic {
    fn predict(&self, _query: &[Itemset]) -> Topic {
        self.cache
    }
}

/// Baseline: the last video's topic. With several topics, the one most common in training.
pub struct LastTopic {
    /// Number of training sessions containing each topic.
    counts: HashMap<Topic, usize>,
}

impl LastTopic {
    pub fn build(train: &[Vec<Itemset>], audit: &Audit) -> Self {
        let mut counts: HashMap<Topic, usize> = HashMap::new();
        for s in train {
            let mut seen: Vec<Topic> = s.iter().flatten().copied().collect();
            seen.sort_unstable();
            seen.dedup();
            for t in seen {
                *counts.entry(t).or_default() += 1;
            }
        }
        let mut ranked: Vec<(&Topic, &usize)> = counts.iter().collect();
        ranked.sort_unstable();
        audit.table("topic_sessions.tsv", "topic\tsessions", ranked.iter().map(|(t, c)| format!("{t}\t{c}")));
        LastTopic { counts }
    }
}

impl Model for LastTopic {
    fn predict(&self, query: &[Itemset]) -> Topic {
        *query[query.len() - 1].iter().max_by_key(|t| (self.counts.get(t).copied().unwrap_or(0), **t)).unwrap()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Agg {
    /// Sum of rule weights.
    Sum,
    /// Strongest single rule.
    Max,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Rules below this support are ignored.
    pub min_support: u32,
    /// Maximum pattern length used, in elements (videos), consequent included.
    pub max_len: usize,
    pub agg: Agg,
    /// Divides each rule's probability by `prior(t)^alpha`; 1 gives lift, 0 plain probability.
    pub alpha: f64,
    /// Weight multiplier per item (topic) in the prefix: above 1 favours longer, more specific rules.
    pub len_weight: f64,
    /// Weight multiplier per video between the prefix's last match and the query's end.
    pub recency: f64,
}

/// For a query, every frequent pattern `<prefix, {t}>` whose prefix is a subsequence of the query yields
/// the rule `prefix -> t` with probability `P(t | prefix) = support(<prefix, {t}>) / support(<prefix>)`.
/// A topic's score combines its rules.
pub struct ProbabilityModel<'a> {
    pub patterns: &'a Patterns,
    pub cfg: Config,
    n_items: usize,
    prior: Vec<f64>,
}

impl<'a> ProbabilityModel<'a> {
    pub fn build(patterns: &'a Patterns, cfg: Config) -> Self {
        let root = &patterns.nodes[0];
        let n_items = patterns.children(root).iter().map(|c| c.item as usize + 1).max().unwrap_or(0);
        let mut prior = vec![0.0; n_items];
        for c in patterns.children(root) {
            prior[c.item as usize] = c.support as f64 / root.support as f64;
        }
        ProbabilityModel { patterns, cfg, n_items, prior }
    }

    /// Scores every topic for a query of at most `cfg.max_len` videos, oldest first.
    pub fn scores(&self, query: &[Itemset]) -> Vec<f64> {
        assert!(query.len() <= self.cfg.max_len);
        let mut item_mask = vec![0u8; self.n_items];
        for (i, itemset) in query.iter().enumerate() {
            for &t in itemset {
                if (t as usize) < self.n_items {
                    item_mask[t as usize] |= 1 << i;
                }
            }
        }
        let mut scores = vec![0.0; self.n_items];
        // Each `mask` holds the query positions where the prefix's last element can end.
        self.visit(&self.patterns.nodes[0], 0, 0, 0, query.len(), &item_mask, &mut scores);
        scores
    }

    /// `prefix_len` counts the prefix's elements (videos), `prefix_items` its items (topics).
    #[allow(clippy::too_many_arguments)]
    fn visit(
        &self,
        node: &Node,
        mask: u8,
        prefix_len: usize,
        prefix_items: usize,
        q_len: usize,
        item_mask: &[u8],
        scores: &mut [f64],
    ) {
        let cfg = &self.cfg;
        let recency = if mask == 0 { 0 } else { q_len - 1 - (7 - mask.leading_zeros() as usize) };
        let weight = cfg.len_weight.powi(prefix_items as i32) * cfg.recency.powi(recency as i32);
        // Bits strictly after the earliest position where the prefix ends.
        let after = if mask == 0 { 0xFF } else { !(mask & mask.wrapping_neg()).wrapping_mul(2).wrapping_sub(1) };

        for c in self.patterns.children(node) {
            if c.support < cfg.min_support {
                continue;
            }
            let t = c.item as usize;
            if c.is_seq && prefix_len < cfg.max_len {
                let p = c.support as f64 / node.support as f64;
                let v = weight * p / self.prior[t].powf(cfg.alpha);
                match cfg.agg {
                    Agg::Sum => scores[t] += v,
                    Agg::Max => scores[t] = scores[t].max(v),
                }
            }
            // A child at `max_len` elements cannot be the prefix of a rule.
            let child_len = prefix_len + c.is_seq as usize;
            if child_len >= cfg.max_len {
                continue;
            }
            let next = if t >= item_mask.len() {
                0
            } else if c.is_seq {
                item_mask[t] & after
            } else {
                item_mask[t] & mask
            };
            if next != 0 {
                self.visit(c, next, child_len, prefix_items + 1, q_len, item_mask, scores);
            }
        }
    }
}

impl Model for ProbabilityModel<'_> {
    /// Highest-scoring topic; ties go to the larger prior, then the smaller id.
    fn predict(&self, query: &[Itemset]) -> Topic {
        let s = self.scores(query);
        let mut best = 0;
        for t in 1..s.len() {
            if s[t] > s[best] || (s[t] == s[best] && self.prior[t] > self.prior[best]) {
                best = t;
            }
        }
        best as Topic
    }
}

/// All (query, ground truth) pairs of a set, with identical queries merged.
pub struct Question {
    pub query: Vec<Itemset>,
    /// How many (query, ground truth) pairs were merged into this question.
    pub pairs: u32,
    /// How many of the query's pairs have each topic in the ground truth.
    pub answers: HashMap<Topic, u32>,
}

/// Creates the questions of a set of sessions once, then scores any number of models on them.
pub struct Evaluator {
    pub total: u64,
    pub questions: Vec<Question>,
}

impl Evaluator {
    /// Each position after a session's first video gives one pair: up to `max_len` nearest previous
    /// videos as the query, the video at that position as the ground truth. `max_len` is the longest
    /// mined pattern of the models to score.
    pub fn build(sessions: &[Vec<Itemset>], max_len: usize, name: &str, audit: &Audit) -> Self {
        let mut merged: HashMap<Vec<Itemset>, (u32, HashMap<Topic, u32>)> = HashMap::new();
        let mut total = 0;
        for s in sessions {
            for i in 1..s.len() {
                let q = &s[i.saturating_sub(max_len)..i];
                let entry = merged.entry(q.to_vec()).or_default();
                entry.0 += 1;
                for &t in &s[i] {
                    *entry.1.entry(t).or_default() += 1;
                }
                total += 1;
            }
        }
        let mut questions: Vec<Question> =
            merged.into_iter().map(|(query, (pairs, answers))| Question { query, pairs, answers }).collect();
        questions.sort_unstable_by(|a, b| a.query.cmp(&b.query));
        audit.log(&format!("pairs_{name}"), total);
        audit.table(
            &format!("questions_{name}.tsv"),
            "query\tpairs\tanswers",
            questions.iter().map(|q| {
                let mut answers: Vec<_> = q.answers.iter().collect();
                answers.sort_unstable();
                let answers: Vec<String> = answers.iter().map(|(t, n)| format!("{t}:{n}")).collect();
                format!("{}\t{}\t{}", audit::sequence(&q.query), q.pairs, answers.join(","))
            }),
        );
        Evaluator { total, questions }
    }

    /// Hit@1: share of pairs whose predicted topic is among the next video's topics.
    pub fn evaluate(&self, model: &impl Model) -> f64 {
        self.hits(&self.predictions(model)) as f64 / self.total as f64
    }

    /// `evaluate`, also exporting the prediction for every question.
    pub fn evaluate_exported(&self, model: &impl Model, name: &str, audit: &Audit) -> f64 {
        let predictions = self.predictions(model);
        audit.table(
            &format!("predictions_{name}.tsv"),
            "query\tpairs\tpredicted\thits",
            self.questions.iter().zip(&predictions).map(|(q, p)| {
                format!("{}\t{}\t{p}\t{}", audit::sequence(&q.query), q.pairs, q.answers.get(p).unwrap_or(&0))
            }),
        );
        let hits = self.hits(&predictions);
        let rate = hits as f64 / self.total as f64;
        audit.log(&format!("hit_rate_{name}"), format!("{hits}/{} = {rate}", self.total));
        rate
    }

    fn predictions(&self, model: &impl Model) -> Vec<Topic> {
        self.questions.par_iter().map(|q| model.predict(&q.query)).collect()
    }

    fn hits(&self, predictions: &[Topic]) -> u64 {
        self.questions.iter().zip(predictions).map(|(q, p)| *q.answers.get(p).unwrap_or(&0) as u64).sum()
    }
}
