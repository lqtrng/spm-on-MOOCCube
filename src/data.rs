//! Loading MOOCCubeX, mapping it to (time, user, video, topics), splitting and sessionizing.
//!
//! - time, user, video: one row per watch segment in `relations/user-video.json`.
//! - topics: what the video covers, see `Dataset::map_topics`.

use crate::audit::{self, Audit};
use rayon::prelude::*;
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const SESSION_GAP_SECS: u32 = 20 * 60;
/// Longer sessions are automated traffic: their count rises again past this length, and 338 of them
/// (over 1000 videos in 20-minute gaps) would otherwise produce most of the evaluation pairs.
pub const MAX_SESSION_VIDEOS: usize = 100;
/// `local_start_time` comes from client clocks. Almost all rows are in 2020; the rest span 2001-2031.
const VALID_TIME: std::ops::Range<u32> = 1_577_836_800..1_609_459_200;

pub type Topic = u16;
pub type Itemset = Vec<Topic>;
pub type Session = Vec<Itemset>;

/// The three MOOCCubeX files, as read. `Segment` leaves out `start_point`, `end_point` and `speed`.
struct RawDataset {
    /// `user-video.json`, one JSON object per line.
    user_videos: Vec<UserVideo>,
    /// `video_id-ccid.txt`, tab-separated: (`V_234845`, ccid). One lecture has many video ids.
    video_ccids: Vec<(String, String)>,
    /// `concept-video.txt`, tab-separated: (`K_<name>_<discipline>`, ccid).
    topic_ccids: Vec<(String, String)>,
}

impl RawDataset {
    /// Reads the three files in `dir`. All of `user-video.json` is held in memory, in no fixed user order.
    pub fn parse(dir: &Path) -> Self {
        RawDataset {
            user_videos: lines(&dir.join("user-video.json"))
                .par_bridge()
                .map(|line| serde_json::from_str(&line).expect("user-video row"))
                .collect(),
            video_ccids: tsv_pairs(&dir.join("video_id-ccid.txt")).collect(),
            topic_ccids: tsv_pairs(&dir.join("concept-video.txt")).collect(),
        }
    }
}

/// From a directory of raw files to a `Dataset`: the only way into this module's data.
pub struct ETL {
    dir: PathBuf,
}

impl ETL {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
        }
    }

    pub fn run(&self, audit: &Audit) -> Dataset {
        Self::transform(self.extract(), audit)
    }

    fn extract(&self) -> RawDataset {
        RawDataset::parse(&self.dir)
    }

    fn transform(raw_dataset: RawDataset, audit: &Audit) -> Dataset {
        Dataset::try_from(raw_dataset, audit).expect("failed to transform")
    }
}

#[derive(Clone, Copy)]
pub struct Event {
    pub ts: u32,
    pub user: u32,
    pub video: u32,
}

pub struct Dataset {
    /// Sorted by (ts, user, video), one row per distinct (ts, user, video).
    pub events: Vec<Event>,
    pub video_topics: HashMap<u32, Itemset>,
    /// Indexed by topic id.
    pub topic_names: Vec<String>,
}

#[derive(Deserialize)]
struct Segment {
    local_start_time: Option<f64>,
}

#[derive(Deserialize)]
struct Watch {
    video_id: String,
    segment: Vec<Segment>,
}

#[derive(Deserialize)]
struct UserVideo {
    user_id: String,
    seq: Vec<Watch>,
}

fn lines(path: &Path) -> impl Iterator<Item = String> {
    let file = File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    BufReader::new(file).lines().map(|l| l.expect("read line")).filter(|l| !l.trim().is_empty())
}

/// Reads `path`, tab-separated, as (first column, second column) pairs.
fn tsv_pairs(path: &Path) -> impl Iterator<Item = (String, String)> {
    lines(path).filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_string(), b.to_string())))
}

/// Topics per video.
const TOPICS_PER_VIDEO: usize = 3;
/// A topic linked to fewer videos identifies one video rather than a subject.
const MIN_VIDEOS_PER_TOPIC: u32 = 5;

/// Name fragments of people and institutions, which say who teaches and not what is taught.
const NOT_CONTENT: [&str; 8] = ["teacher", "professor", "college", "university", "institute", "academy", "school", "mr. "];
/// Words of the dialogue the translation model invents for short terms ("i'm sorry.", "huh?").
const DIALOGUE: [&str; 15] = [
    "i'm", "you", "you're", "sorry", "oh", "yeah", "huh", "okay", "hey", "please", "gonna", "shit", "fuck", "damn", "guy",
];
/// Longer names are sentences or repetitions from the translation model, not terms.
const MAX_WORDS_PER_TOPIC: usize = 6;

impl Dataset {
    /// (train, val, test): a 70/15/15 split of the users, so that no user appears in two sets. Users are
    /// assigned by a hash of their id, so the assignment is deterministic and independent of the data.
    pub fn split_by_user(&self) -> (Vec<Event>, Vec<Event>, Vec<Event>) {
        let (mut train, mut val, mut test) = (Vec::new(), Vec::new(), Vec::new());
        for e in &self.events {
            let bucket = (e.user as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32;
            match bucket % 100 {
                0..70 => train.push(*e),
                70..85 => val.push(*e),
                _ => test.push(*e),
            }
        }
        (train, val, test)
    }

    /// Maps the raw files to (time, user, video, topics) and cleans the rows.
    fn try_from(raw: RawDataset, audit: &Audit) -> io::Result<Self> {
        audit.log("raw_users", raw.user_videos.len());
        audit.log("raw_video_ccids", raw.video_ccids.len());
        audit.log("raw_topic_ccids", raw.topic_ccids.len());
        let (video_topics, topic_names) = Self::map_topics(&raw, audit)?;
        let per_user: Vec<Vec<Event>> =
            raw.user_videos.par_iter().map(|row| Self::user_events(row, &video_topics)).collect::<Result<_, _>>()?;
        drop(raw);
        let mut events: Vec<Event> = per_user.into_iter().flatten().collect();
        audit.log("events_in_valid_time", events.len());
        events.par_sort_unstable_by_key(|e| (e.ts, e.user, e.video));
        events.dedup_by_key(|e| (e.ts, e.user, e.video));
        audit.log("events_distinct", events.len());
        let events = Self::drop_long_sessions(events);
        audit.log("events_without_long_sessions", events.len());
        Ok(Dataset { events, video_topics, topic_names })
    }

    /// One video exists under many `video_id`s (one per course run) sharing a `ccid`. A video keeps its
    /// `TOPICS_PER_VIDEO` most specific usable topics: those linked to the fewest videos, which avoids
    /// generic words such as method or requirement. This is measured on the links in `concept-video.txt`,
    /// not on watch behaviour.
    fn map_topics(raw: &RawDataset, audit: &Audit) -> io::Result<(HashMap<u32, Itemset>, Vec<String>)> {
        let mut topics: HashMap<&str, BTreeSet<String>> = HashMap::new();
        for (topic, ccid) in &raw.topic_ccids {
            if let Some(topic) = Self::clean_topic(topic) {
                topics.entry(ccid).or_default().insert(topic);
            }
        }
        let mut videos_with: HashMap<String, u32> = HashMap::new();
        for t in topics.values().flatten() {
            *videos_with.entry(t.clone()).or_default() += 1;
        }
        let chosen: HashMap<&str, Vec<&str>> = topics
            .iter()
            .map(|(ccid, ts)| {
                let mut ts: Vec<&str> =
                    ts.iter().map(String::as_str).filter(|t| videos_with[*t] >= MIN_VIDEOS_PER_TOPIC).collect();
                ts.sort_by_key(|t| (videos_with[*t], *t));
                ts.truncate(TOPICS_PER_VIDEO);
                (*ccid, ts)
            })
            .filter(|(_, ts)| !ts.is_empty())
            .collect();

        let mut names: Vec<String> = chosen.values().flatten().map(|t| t.to_string()).collect();
        names.sort();
        names.dedup();
        if names.len() > Topic::MAX as usize {
            return Err(io::Error::other(format!("too many topics: {}", names.len())));
        }
        let ids: HashMap<&str, Topic> = names.iter().enumerate().map(|(i, n)| (n.as_str(), i as Topic)).collect();

        // Every usable topic, with an empty id when no video kept it.
        let mut linked: Vec<(&String, &u32)> = videos_with.iter().collect();
        linked.sort_unstable();
        audit.table(
            "topics.tsv",
            "topic_id\tvideos_linked\tname",
            linked.iter().map(|(name, n)| {
                let id = ids.get(name.as_str()).map_or(String::new(), Topic::to_string);
                format!("{id}\t{n}\t{name}")
            }),
        );

        let mut video_topics = HashMap::new();
        for (v, ccid) in &raw.video_ccids {
            if let (Some(video), Some(ts)) = (Self::numeric_id(v), chosen.get(ccid.as_str())) {
                let mut topics: Itemset = ts.iter().map(|t: &&str| ids[t]).collect();
                topics.sort_unstable();
                video_topics.insert(video, topics);
            }
        }
        let mut videos: Vec<(&u32, &Itemset)> = video_topics.iter().collect();
        videos.sort_unstable();
        audit.table("video_topics.tsv", "video\ttopics", videos.iter().map(|(v, ts)| format!("{v}\t{}", audit::itemset(ts))));
        Ok((video_topics, names))
    }

    /// One row per watch segment of `row` in `VALID_TIME`, for videos that have topics.
    fn user_events(row: &UserVideo, video_topics: &HashMap<u32, Itemset>) -> io::Result<Vec<Event>> {
        let user = Self::numeric_id(&row.user_id)
            .ok_or_else(|| io::Error::other(format!("bad user id: {}", row.user_id)))?;
        let mut out = Vec::new();
        for w in &row.seq {
            let Some(video) = Self::numeric_id(&w.video_id).filter(|v| video_topics.contains_key(v)) else {
                continue;
            };
            for s in &w.segment {
                if let Some(ts) = s.local_start_time.map(|t| t as u32).filter(|t| VALID_TIME.contains(t)) {
                    out.push(Event { ts, user, video });
                }
            }
        }
        Ok(out)
    }

    /// A topic id looks like `K_function_Mathematics`; the name may itself contain `_`. Returns the id
    /// without the punctuation translation left around the name, so "binary." and "binary" are one
    /// topic, or None if the name is not a usable topic.
    fn clean_topic(topic: &str) -> Option<String> {
        let (name, discipline) = topic.strip_prefix("K_")?.rsplit_once('_')?;
        let name = name
            .trim_start_matches(['-', ',', '.', '"', '&', '_', ')', ' '])
            .trim_end_matches(['.', ',', '!', '?', ':', ';', '"', '-', '_', ' ']);
        Self::is_real_topic(name).then(|| format!("K_{name}_{discipline}"))
    }

    fn is_real_topic(name: &str) -> bool {
        let mut words = name.split(|c: char| !c.is_alphanumeric() && c != '\'');
        name.chars().count() >= 2
            && name.chars().any(char::is_alphabetic)
            && !name.contains('\u{266a}')
            && !name.starts_with('#')
            && !NOT_CONTENT.iter().any(|w| name.contains(w))
            && name.split_whitespace().count() <= MAX_WORDS_PER_TOPIC
            && !words.any(|w| DIALOGUE.contains(&w))
    }

    fn numeric_id(id: &str) -> Option<u32> {
        id.split_once('_').and_then(|(_, n)| n.parse().ok())
    }

    fn drop_long_sessions(events: Vec<Event>) -> Vec<Event> {
        let mut kept: Vec<Event> = by_user(&events)
            .iter()
            .flat_map(|timeline| session_rows(timeline).collect::<Vec<_>>())
            .filter(|rows| session_videos(rows).len() <= MAX_SESSION_VIDEOS)
            .flatten()
            .copied()
            .collect();
        kept.par_sort_unstable_by_key(|e| (e.ts, e.user, e.video));
        kept
    }
}

fn by_user(events: &[Event]) -> Vec<Vec<Event>> {
    let mut map: HashMap<u32, Vec<Event>> = HashMap::new();
    for e in events {
        map.entry(e.user).or_default().push(*e);
    }
    let mut users: Vec<_> = map.into_iter().collect();
    users.sort_unstable_by_key(|(u, _)| *u);
    users.into_iter().map(|(_, timeline)| timeline).collect()
}

/// Cuts one user's timeline wherever the gap exceeds `SESSION_GAP_SECS`, as slices of rows.
fn session_rows(timeline: &[Event]) -> impl Iterator<Item = &[Event]> {
    timeline.chunk_by(|a, b| b.ts - a.ts <= SESSION_GAP_SECS)
}

/// Rows of one session as its videos. Further segments of the video just watched are the same
/// video, not a "next video".
fn session_videos(rows: &[Event]) -> Vec<Event> {
    let mut videos = rows.to_vec();
    videos.dedup_by_key(|e| e.video);
    videos
}

/// Groups by user first, then by session; each session is the list of videos watched.
pub fn sessions(events: &[Event]) -> Vec<Vec<Event>> {
    by_user(events).iter().flat_map(|timeline| session_rows(timeline).map(session_videos).collect::<Vec<_>>()).collect()
}

pub fn to_itemsets(session: &[Event], video_topics: &HashMap<u32, Itemset>) -> Session {
    session.iter().map(|e| video_topics[&e.video].clone()).collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn clean(topic: &str) -> Option<String> {
        Dataset::clean_topic(topic)
    }

    #[test]
    fn topic_names_are_normalized() {
        assert_eq!(clean("K_binary._Computer Science").as_deref(), Some("K_binary_Computer Science"));
        assert_eq!(clean("K_- polymorphism,_Computer Science").as_deref(), Some("K_polymorphism_Computer Science"));
        assert_eq!(clean("K_foo_bar_Physics").as_deref(), Some("K_foo_bar_Physics"));
        assert_eq!(clean("K_c++_Computer Science").as_deref(), Some("K_c++_Computer Science"));
    }

    #[test]
    fn noise_is_not_a_topic() {
        for topic in [
            "K_i'm sorry._Chinese Language & Literature",
            "K_huh?_Physics",
            "K_\u{266a} can you get a bus \u{266a}_Arts",
            "K_# i go #_Arts",
            "K_2_Pharmacy",
            "K_university of edinburgh_Computer Science",
            "K_mooc's teacher._Education",
            "K_one two three four five six seven_Physics",
            "not a topic",
        ] {
            assert_eq!(clean(topic), None, "{topic}");
        }
    }
}
