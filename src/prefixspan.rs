//! PrefixSpan over sequences of itemsets, with pseudo-projection.
//!
//! Finds every frequent subsequence (not just maximal ones) up to `max_len` elements (itemsets, i.e.
//! videos), whatever the number of items in each element. Support is the
//! number of sequences containing the pattern. Patterns are returned as a prefix tree: PrefixSpan's
//! recursion already visits them that way, and the recommender needs "all extensions of a prefix".
//!
//! A projection keeps, per sequence, every element index where the pattern's last element can end
//! (`ends`). Keeping only the earliest match would miss itemset extensions: in `<(a)(a b)>` the
//! pattern `<a>` first matches element 0, yet `<(a b)>` only embeds at element 1.

use crate::data::{Session, Topic};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Mining stops as soon as either limit is passed, so a too-low support cannot exhaust the machine.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_patterns: u64,
    pub max_rss_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_patterns: 10_000_000, max_rss_bytes: 12 << 30 }
    }
}

#[derive(Debug, PartialEq)]
pub enum MineError {
    TooManyPatterns { limit: u64 },
    TooMuchMemory { limit: u64 },
}

/// Resident memory of this process, from /proc. None where that is unavailable.
fn rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    Some(statm.split_whitespace().nth(1)?.parse::<u64>().ok()? * 4096)
}

/// How many new patterns may pass between two memory readings.
const RSS_CHECK_EVERY: u64 = 1 << 16;

struct Guard {
    limits: Limits,
    patterns: AtomicU64,
    stop: AtomicBool,
    too_much_memory: AtomicBool,
}

impl Guard {
    fn new(limits: Limits) -> Self {
        Guard {
            limits,
            patterns: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            too_much_memory: AtomicBool::new(false),
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn add_patterns(&self, n: u64) {
        let before = self.patterns.fetch_add(n, Ordering::Relaxed);
        if before + n > self.limits.max_patterns {
            self.stop.store(true, Ordering::Relaxed);
        }
        if before / RSS_CHECK_EVERY != (before + n) / RSS_CHECK_EVERY
            && rss_bytes().is_some_and(|rss| rss > self.limits.max_rss_bytes)
        {
            self.too_much_memory.store(true, Ordering::Relaxed);
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn result(&self) -> Result<(), MineError> {
        if self.too_much_memory.load(Ordering::Relaxed) {
            Err(MineError::TooMuchMemory { limit: self.limits.max_rss_bytes })
        } else if self.stopped() {
            Err(MineError::TooManyPatterns { limit: self.limits.max_patterns })
        } else {
            Ok(())
        }
    }
}

/// One pattern: its parent's pattern extended by `item`.
#[derive(Clone, Copy)]
pub struct Node {
    pub item: Topic,
    /// true: `item` starts a new element (sequence extension); false: joins the last element.
    pub is_seq: bool,
    pub support: u32,
    pub first_child: u32,
    pub n_children: u32,
}

/// Flat prefix tree. Node 0 is the empty pattern, whose support is the database size.
pub struct Patterns {
    pub nodes: Vec<Node>,
}

impl Patterns {
    pub fn children(&self, node: &Node) -> &[Node] {
        let start = node.first_child as usize;
        &self.nodes[start..start + node.n_children as usize]
    }
}

struct Entry {
    item: Topic,
    start: u32,
    end: u32,
}

/// Inverted form of the database: per sequence, the sorted positions of each distinct item.
struct Db {
    seq_offsets: Vec<u32>,
    entries: Vec<Entry>,
    positions: Vec<u16>,
    n_items: usize,
}

impl Db {
    fn new(sequences: &[Session]) -> Self {
        let n_items = sequences.iter().flatten().flatten().map(|&t| t as usize + 1).max().unwrap_or(0);
        let mut db = Db { seq_offsets: vec![0], entries: Vec::new(), positions: Vec::new(), n_items };
        let mut occurrences: Vec<Vec<u16>> = vec![Vec::new(); n_items];
        for seq in sequences {
            for (pos, itemset) in seq.iter().enumerate() {
                let pos = u16::try_from(pos).expect("session longer than u16::MAX");
                for &t in itemset {
                    occurrences[t as usize].push(pos);
                }
            }
            for (item, occ) in occurrences.iter_mut().enumerate() {
                if !occ.is_empty() {
                    let start = db.positions.len() as u32;
                    db.positions.extend_from_slice(occ);
                    db.entries.push(Entry { item: item as Topic, start, end: db.positions.len() as u32 });
                    occ.clear();
                }
            }
            db.seq_offsets.push(db.entries.len() as u32);
        }
        db
    }

    fn seq(&self, i: u32) -> &[Entry] {
        &self.entries[self.seq_offsets[i as usize] as usize..self.seq_offsets[i as usize + 1] as usize]
    }

    fn pos(&self, e: &Entry) -> &[u16] {
        &self.positions[e.start as usize..e.end as usize]
    }
}

struct Proj {
    seq: u32,
    start: u32,
    end: u32,
}

#[derive(Default)]
struct Projection {
    seqs: Vec<Proj>,
    ends: Vec<u16>,
}

impl Projection {
    fn push(&mut self, seq: u32, ends: impl Iterator<Item = u16>) {
        let start = self.ends.len() as u32;
        self.ends.extend(ends);
        self.seqs.push(Proj { seq, start, end: self.ends.len() as u32 });
    }
}

struct Tree {
    item: Topic,
    is_seq: bool,
    support: u32,
    children: Vec<Tree>,
}

struct Miner<'a> {
    db: &'a Db,
    min_support: u32,
    max_len: usize,
    guard: &'a Guard,
}

fn intersects(a: &[u16], b: &[u16]) -> bool {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => return true,
        }
    }
    false
}

fn intersection<'a>(a: &'a [u16], b: &'a [u16]) -> impl Iterator<Item = u16> + 'a {
    let (mut i, mut j) = (0, 0);
    std::iter::from_fn(move || {
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                    return Some(a[i - 1]);
                }
            }
        }
        None
    })
}

impl Miner<'_> {
    /// Finds the frequent one-item extensions of a pattern of `len` elements and recurses into them.
    /// A sequence extension adds an element, so it is only tried below `max_len`; an itemset extension
    /// keeps the length, so it is always tried.
    fn grow(&self, proj: &Projection, last_item: Topic, len: usize) -> Vec<Tree> {
        if self.guard.stopped() {
            return Vec::new();
        }
        // Key: 2 * item for a sequence extension, 2 * item + 1 for an itemset extension. Counting by
        // sorting stays cheap with a large vocabulary, where a counter per item would not.
        let can_add_element = len < self.max_len;
        let mut keys: Vec<u32> = Vec::new();
        for p in &proj.seqs {
            let ends = &proj.ends[p.start as usize..p.end as usize];
            for e in self.db.seq(p.seq) {
                let pos = self.db.pos(e);
                if can_add_element && pos[pos.len() - 1] > ends[0] {
                    keys.push(2 * e.item as u32);
                }
                if e.item > last_item && intersects(ends, pos) {
                    keys.push(2 * e.item as u32 + 1);
                }
            }
        }
        keys.sort_unstable();
        let frequent: Vec<(u32, u32)> = keys
            .chunk_by(|a, b| a == b)
            .map(|run| (run[0], run.len() as u32))
            .filter(|&(_, count)| count >= self.min_support)
            .collect();
        drop(keys);
        self.guard.add_patterns(frequent.len() as u64);
        let item_of = |k: u32| (k / 2) as Topic;
        let to_tree = |(k, count): (u32, u32), children| Tree {
            item: item_of(k),
            is_seq: k % 2 == 0,
            support: count,
            children,
        };

        if frequent.is_empty() {
            return Vec::new();
        }

        let slot: HashMap<u32, usize> = frequent.iter().enumerate().map(|(s, &(k, _))| (k, s)).collect();
        let mut children: Vec<Projection> = frequent.iter().map(|_| Projection::default()).collect();
        for p in &proj.seqs {
            let ends = &proj.ends[p.start as usize..p.end as usize];
            for e in self.db.seq(p.seq) {
                let pos = self.db.pos(e);
                if can_add_element && pos[pos.len() - 1] > ends[0] {
                    if let Some(&s) = slot.get(&(2 * e.item as u32)) {
                        let from = pos.partition_point(|&x| x <= ends[0]);
                        children[s].push(p.seq, pos[from..].iter().copied());
                    }
                }
                if e.item > last_item {
                    if let Some(&s) = slot.get(&(2 * e.item as u32 + 1)) {
                        if intersects(ends, pos) {
                            children[s].push(p.seq, intersection(ends, pos));
                        }
                    }
                }
            }
        }

        let recurse = |(&f, child): (&(u32, u32), &Projection)| {
            to_tree(f, self.grow(child, item_of(f.0), len + (f.0 % 2 == 0) as usize))
        };
        if len <= 2 {
            frequent.par_iter().zip(children.par_iter()).map(recurse).collect()
        } else {
            frequent.iter().zip(children.iter()).map(recurse).collect()
        }
    }

    fn root(&self) -> Vec<Tree> {
        let n = self.db.n_items;
        let mut per_item: Vec<Projection> = (0..n).map(|_| Projection::default()).collect();
        for seq in 0..self.db.seq_offsets.len() as u32 - 1 {
            for e in self.db.seq(seq) {
                per_item[e.item as usize].push(seq, self.db.pos(e).iter().copied());
            }
        }
        self.guard.add_patterns(per_item.iter().filter(|p| p.seqs.len() as u32 >= self.min_support).count() as u64);
        per_item
            .par_iter()
            .enumerate()
            .filter(|(_, p)| p.seqs.len() as u32 >= self.min_support)
            .map(|(item, p)| Tree {
                item: item as Topic,
                is_seq: true,
                support: p.seqs.len() as u32,
                children: self.grow(p, item as Topic, 1),
            })
            .collect()
    }
}

pub fn mine(sequences: &[Session], min_support: u32, max_len: usize, limits: Limits) -> Result<Patterns, MineError> {
    let db = Db::new(sequences);
    let guard = Guard::new(limits);
    let miner = Miner { db: &db, min_support, max_len, guard: &guard };
    let top = miner.root();
    guard.result()?;

    // Breadth-first flattening keeps each node's children contiguous.
    let mut nodes = vec![Node {
        item: 0,
        is_seq: true,
        support: sequences.len() as u32,
        first_child: 1,
        n_children: top.len() as u32,
    }];
    let mut level: Vec<Tree> = top;
    while !level.is_empty() {
        let mut next_first = nodes.len() as u32 + level.len() as u32;
        let mut next = Vec::new();
        for t in level {
            let n_children = t.children.len() as u32;
            nodes.push(Node {
                item: t.item,
                is_seq: t.is_seq,
                support: t.support,
                first_child: next_first,
                n_children,
            });
            next_first += n_children;
            next.extend(t.children);
        }
        level = next;
    }
    Ok(Patterns { nodes })
}

/// Mines at `min_support`, doubling it whenever a limit stops the run. Returns the support used.
pub fn mine_within_limits(sequences: &[Session], min_support: u32, max_len: usize, limits: Limits) -> (Patterns, u32) {
    let mut support = min_support.max(1);
    loop {
        match mine(sequences, support, max_len, limits) {
            Ok(patterns) => return (patterns, support),
            Err(e) => {
                println!("min_support={support} stopped by guardrail ({e:?}), retrying with {}", 2 * support);
                support *= 2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    type Pattern = Vec<Vec<Topic>>;

    fn walk(p: &Patterns, node: &Node, cur: &mut Pattern, out: &mut BTreeMap<Pattern, u32>) {
        for c in p.children(node) {
            let mut next = cur.clone();
            if c.is_seq {
                next.push(vec![c.item]);
            } else {
                next.last_mut().unwrap().push(c.item);
            }
            out.insert(next.clone(), c.support);
            walk(p, c, &mut next, out);
        }
    }

    const NO_LIMITS: Limits = Limits { max_patterns: u64::MAX, max_rss_bytes: u64::MAX };

    fn mine_all(db: &[Session], min_support: u32, max_len: usize) -> Patterns {
        mine(db, min_support, max_len, NO_LIMITS).unwrap()
    }

    /// Every session holds the same 12 topics, so the pattern count explodes at any support.
    fn explosive_db() -> Vec<Session> {
        let session: Session = (0..12).map(|t| vec![t as Topic]).collect();
        vec![session; 3]
    }

    #[test]
    fn pattern_limit_stops_mining() {
        let limits = Limits { max_patterns: 1000, max_rss_bytes: u64::MAX };
        assert_eq!(
            mine(&explosive_db(), 1, 7, limits).err(),
            Some(MineError::TooManyPatterns { limit: 1000 })
        );
    }

    #[test]
    fn memory_limit_stops_mining() {
        let limits = Limits { max_patterns: u64::MAX, max_rss_bytes: 1 };
        let mut db = explosive_db();
        db[0] = (0..30).map(|t| vec![t as Topic]).collect();
        db[1] = db[0].clone();
        assert_eq!(mine(&db, 1, 7, limits).err(), Some(MineError::TooMuchMemory { limit: 1 }));
    }

    #[test]
    fn backoff_raises_support_until_the_limit_holds() {
        // Sessions of decreasing size: support 1 explodes, support 8 leaves a small database.
        let mut db: Vec<Session> = Vec::new();
        for _ in 0..8 {
            db.push((0..3).map(|t| vec![t as Topic]).collect());
        }
        db.push((0..14).map(|t| vec![t as Topic]).collect());
        let limits = Limits { max_patterns: 100, max_rss_bytes: u64::MAX };
        let (found, support) = mine_within_limits(&db, 1, 7, limits);
        assert!(support > 1);
        assert!(found.nodes.len() - 1 <= 100);
        assert_eq!(all_patterns(&found), all_patterns(&mine_all(&db, support, 7)));
    }

    fn all_patterns(p: &Patterns) -> BTreeMap<Pattern, u32> {
        let mut out = BTreeMap::new();
        walk(p, &p.nodes[0], &mut Vec::new(), &mut out);
        out
    }

    fn contains(seq: &Session, pat: &Pattern) -> bool {
        let mut i = 0;
        for element in pat {
            while i < seq.len() && !element.iter().all(|x| seq[i].contains(x)) {
                i += 1;
            }
            if i == seq.len() {
                return false;
            }
            i += 1;
        }
        true
    }

    /// Enumerates every pattern up to `max_len` elements and counts support directly.
    fn brute_force(db: &[Session], min_support: u32, max_len: usize) -> BTreeMap<Pattern, u32> {
        let items: BTreeSet<Topic> = db.iter().flatten().flatten().copied().collect();
        let mut out = BTreeMap::new();
        let mut frontier: Vec<Pattern> = vec![vec![]];
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for pat in &frontier {
                for &x in &items {
                    if pat.len() < max_len {
                        let mut seq_ext = pat.clone();
                        seq_ext.push(vec![x]);
                        next.push(seq_ext);
                    }
                    if let Some(last) = pat.last() {
                        if *last.last().unwrap() < x {
                            let mut item_ext = pat.clone();
                            item_ext.last_mut().unwrap().push(x);
                            next.push(item_ext);
                        }
                    }
                }
            }
            next.retain(|p| {
                let s = db.iter().filter(|q| contains(q, p)).count() as u32;
                if s >= min_support {
                    out.insert(p.clone(), s);
                    true
                } else {
                    false
                }
            });
            frontier = next;
        }
        out
    }

    #[test]
    fn textbook_example() {
        // The running example of the PrefixSpan paper (Pei et al.), a=1 ... g=7.
        let db: Vec<Session> = vec![
            vec![vec![1], vec![1, 2, 3], vec![1, 3], vec![4], vec![3, 6]],
            vec![vec![1, 4], vec![3], vec![2, 3], vec![1, 5]],
            vec![vec![5, 6], vec![1, 2], vec![4, 6], vec![3], vec![2]],
            vec![vec![5], vec![7], vec![1, 6], vec![3], vec![2], vec![3]],
        ];
        let found = all_patterns(&mine_all(&db, 2, 10));
        // The paper reports 53 frequent sequences at min_support = 2.
        assert_eq!(found.len(), 53);
        assert_eq!(found[&vec![vec![1, 2], vec![3]]], 2);
        assert_eq!(found[&vec![vec![1], vec![2, 3], vec![1]]], 2);
        assert_eq!(found, brute_force(&db, 2, 10));
    }

    #[test]
    fn max_len_counts_elements_not_items() {
        let db: Vec<Session> = vec![vec![vec![1, 2, 3], vec![4, 5]]; 2];
        let found = all_patterns(&mine_all(&db, 2, 2));
        assert_eq!(found[&vec![vec![1, 2, 3], vec![4, 5]]], 2);
        assert!(found.keys().all(|p| p.len() <= 2));
        let one = all_patterns(&mine_all(&db, 2, 1));
        assert_eq!(one[&vec![vec![1, 2, 3]]], 2);
        assert!(one.keys().all(|p| p.len() == 1));
    }

    #[test]
    fn itemset_extension_uses_later_match() {
        let db: Vec<Session> = vec![vec![vec![1], vec![1, 2]], vec![vec![1], vec![1, 2]]];
        let found = all_patterns(&mine_all(&db, 2, 5));
        assert_eq!(found[&vec![vec![1, 2]]], 2);
        assert_eq!(found[&vec![vec![1], vec![1, 2]]], 2);
    }

    #[test]
    fn matches_brute_force_on_random_databases() {
        // Small deterministic LCG, to stay free of a test-only dependency.
        let mut state = 12345u64;
        let mut rnd = |m: u64| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) % m
        };
        for _ in 0..40 {
            let db: Vec<Session> = (0..12)
                .map(|_| {
                    (0..rnd(6))
                        .map(|_| {
                            let mut s: Vec<Topic> = (0..1 + rnd(3)).map(|_| rnd(5) as Topic).collect();
                            s.sort_unstable();
                            s.dedup();
                            s
                        })
                        .collect()
                })
                .collect();
            for (min_support, max_len) in [(2, 7), (3, 3), (5, 2)] {
                assert_eq!(
                    all_patterns(&mine_all(&db, min_support, max_len)),
                    brute_force(&db, min_support, max_len)
                );
            }
        }
    }
}
