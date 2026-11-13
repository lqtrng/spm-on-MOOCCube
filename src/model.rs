//! Next-topic models, the probability model and the baselines, and their evaluation.

use crate::audit::{self, Audit};
use crate::data::{self, Dataset, Event, Itemset, Topic};
use crate::prefixspan::{Node, Patterns};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

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

/// A path steps from `x` to `t` only if `t` comes after `x` in at least this share of the sessions with `x`:
/// for a very common topic, a rare coincidence is still a frequent pattern. Mirrors MIN_LINK in `src/planner.html`.
pub const MIN_LINK: f64 = 0.05;

/// Query positions are tracked in the bits of a `u8`.
pub const MAX_QUERY: usize = 8;

/// For a query, every frequent pattern `<prefix, {t}>` whose prefix is a subsequence of the query yields
/// the rule `prefix -> t` with probability `P(t | prefix) = support(<prefix, {t}>) / support(<prefix>)`.
/// A topic's score combines its rules.
pub struct ProbabilityModel<'a> {
    pub patterns: &'a Patterns,
    pub cfg: Config,
    n_items: usize,
    prior: Vec<f64>,
    /// Index of each topic's pattern `<(t)>` in `patterns.nodes`.
    root_node: Vec<Option<u32>>,
    /// Topics by falling prior, then id.
    by_prior: Vec<Topic>,
}

impl<'a> ProbabilityModel<'a> {
    pub fn build(patterns: &'a Patterns, cfg: Config) -> Self {
        let root = &patterns.nodes[0];
        let n_items = patterns.children(root).iter().map(|c| c.item as usize + 1).max().unwrap_or(0);
        let mut prior = vec![0.0; n_items];
        let mut root_node = vec![None; n_items];
        for (i, c) in patterns.children(root).iter().enumerate() {
            prior[c.item as usize] = c.support as f64 / root.support as f64;
            root_node[c.item as usize] = Some(root.first_child + i as u32);
        }
        let mut by_prior: Vec<Topic> = patterns.children(root).iter().map(|c| c.item).collect();
        by_prior.sort_unstable_by(|a, b| prior[*b as usize].total_cmp(&prior[*a as usize]).then(a.cmp(b)));
        ProbabilityModel { patterns, cfg, n_items, prior, root_node, by_prior }
    }

    /// A study path: the most likely topic at each step, each chosen topic joining the context that
    /// predicts the one after. Every topic has some score from its prior alone, so a step only goes to a
    /// topic linked to the context by a pattern `<(x)(t)>` of weight above zero that holds in `MIN_LINK`
    /// of the sessions with `x`, and never to one in `seen`. At recency 0 only patterns whose prefix ends at
    /// the last video have weight, so `x` must be in the last video. Stops early when nothing qualifies.
    /// The context holds the `query_len` nearest videos.
    pub fn rollout(&self, query: &[Itemset], seen: &mut HashSet<Topic>, steps: usize, query_len: usize) -> Vec<Topic> {
        let mut context = query.to_vec();
        let mut path = Vec::new();
        for _ in 0..steps {
            let s = self.scores(&context);
            let mut best: Option<usize> = None;
            let sources = if self.cfg.recency > 0.0 { &context[..] } else { &context[context.len().saturating_sub(1)..] };
            for &x in sources.iter().flatten() {
                let Some(node) = self.root_node.get(x as usize).copied().flatten() else { continue };
                let node = &self.patterns.nodes[node as usize];
                for c in self.patterns.children(node) {
                    let t = c.item as usize;
                    let linked = c.is_seq && c.item != x && c.support as f64 >= MIN_LINK * node.support as f64;
                    if !linked || seen.contains(&c.item) {
                        continue;
                    }
                    // Ties go to the larger prior, then the smaller id.
                    let better = best.is_none_or(|b| {
                        s[t] > s[b] || (s[t] == s[b] && (self.prior[t] > self.prior[b] || (self.prior[t] == self.prior[b] && t < b)))
                    });
                    if better {
                        best = Some(t);
                    }
                }
            }
            let Some(best) = best else { break };
            let best = best as Topic;
            path.push(best);
            seen.insert(best);
            context.push(vec![best]);
            if context.len() > query_len {
                context.remove(0);
            }
        }
        path
    }

    /// Baseline path: the topics in the most training sessions, skipping those in `seen`.
    pub fn popular_path(&self, seen: &HashSet<Topic>, steps: usize) -> Vec<Topic> {
        self.by_prior.iter().copied().filter(|t| !seen.contains(t)).take(steps).collect()
    }

    /// Scores every topic for a query of at most `MAX_QUERY` videos, oldest first. A prefix may match any
    /// videos of the query in order, so a query longer than the longest prefix still adds matches.
    pub fn scores(&self, query: &[Itemset]) -> Vec<f64> {
        assert!(query.len() <= MAX_QUERY);
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

/// How good planned paths are, per step: at every point of every session a path is planned from the
/// videos so far, and step k is a hit if its topic is in a later video of that session.
pub struct PathScore {
    /// Points at which a path was asked for.
    pub points: u64,
    /// Per step: how many paths got that far, and how many of those steps were hits.
    pub planned: Vec<u64>,
    pub hits: Vec<u64>,
}

impl PathScore {
    /// `plan` gets every topic watched so far in the session: the planner page treats what the user has
    /// learned as one set, without order.
    pub fn build(
        sessions: &[Vec<Itemset>],
        steps: usize,
        plan: impl Fn(HashSet<Topic>) -> Vec<Topic> + Sync,
    ) -> Self {
        let zero = || PathScore { points: 0, planned: vec![0; steps], hits: vec![0; steps] };
        sessions
            .par_iter()
            .map(|s| {
                let mut score = zero();
                for i in 1..s.len() {
                    let seen: HashSet<Topic> = s[..i].iter().flatten().copied().collect();
                    let later: HashSet<Topic> = s[i..].iter().flatten().copied().collect();
                    score.points += 1;
                    for (k, t) in plan(seen).iter().enumerate() {
                        score.planned[k] += 1;
                        score.hits[k] += later.contains(t) as u64;
                    }
                }
                score
            })
            .reduce(zero, |mut a, b| {
                a.points += b.points;
                for k in 0..steps {
                    a.planned[k] += b.planned[k];
                    a.hits[k] += b.hits[k];
                }
                a
            })
    }

    pub fn export(&self, name: &str, audit: &Audit) {
        audit.log(&format!("path_points_{name}"), self.points);
        for k in 0..self.planned.len() {
            audit.log(&format!("path_step{}_{name}", k + 1), format!("{}/{} hits/planned", self.hits[k], self.planned[k]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefixspan::{self, Limits};

    #[test]
    fn rollout_follows_links_and_skips_seen_topics() {
        // 1 -> 2 -> 3 three times; topic 9 is the most common but follows nothing.
        // 1 -> 4 in 2 of the 32 sessions with 1, 1 -> 5 in only 1: below MIN_LINK.
        let mut db = vec![vec![vec![1], vec![2], vec![3]]; 3];
        db.extend(vec![vec![vec![1], vec![4]]; 2]);
        db.push(vec![vec![1], vec![5]]);
        db.extend(vec![vec![vec![1], vec![2]]; 26]);
        db.extend(vec![vec![vec![9]]; 40]);
        let limits = Limits { max_patterns: u64::MAX, max_rss_bytes: u64::MAX };
        let patterns = prefixspan::mine(&db, 1, 3, limits).unwrap();
        let cfg = Config { min_support: 1, max_len: 3, agg: Agg::Sum, alpha: 0.0, len_weight: 4.0, recency: 0.0 };
        let model = ProbabilityModel::build(&patterns, cfg);

        // Stops after 3, which nothing follows. 4 only follows 1, which is no longer the last video.
        assert_eq!(model.rollout(&[vec![1]], &mut HashSet::from([1]), 6, 6), vec![2, 3]);
        assert_eq!(model.rollout(&[vec![1]], &mut HashSet::from([1, 2]), 6, 6), vec![3]);
        assert_eq!(model.rollout(&[vec![1]], &mut HashSet::from([1, 2, 3, 4]), 6, 6), Vec::<Topic>::new());
        let lenient = ProbabilityModel::build(&patterns, Config { recency: 0.5, ..cfg });
        assert_eq!(lenient.rollout(&[vec![1]], &mut HashSet::from([1]), 6, 6), vec![2, 3, 4]);
        assert_eq!(model.rollout(&[vec![9]], &mut HashSet::from([9]), 6, 6), Vec::<Topic>::new());
        // Learned topics as one video: 9 leads nowhere, 1 does.
        assert_eq!(model.rollout(&[vec![1, 9]], &mut HashSet::from([1, 9]), 6, 6), vec![2, 3]);
        assert_eq!(model.popular_path(&HashSet::from([9]), 2), vec![1, 2]);
        assert_eq!(model.popular_path(&HashSet::new(), 1), vec![9]);
    }
}
