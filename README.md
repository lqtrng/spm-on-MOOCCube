# Next-topic recommender (PrefixSpan on MOOCCubeX)

Predicts the topic of a learner's next video from the last videos of their session, as many as the longest mined
pattern (3 videos), using simple probabilities over frequent patterns. `demo.html` is the same system in one self-contained
page: open it in a browser, pick topics, read the prediction.

## Results (test set: 77,240 pairs from users not in training)

| | Hit@1 |
|---|---|
| **Pattern model** | **68.61%** |
| Baseline: always the last topic | 17.04% |
| Baseline: always the topic with most views | 9.10% |

**Limit:** on a chronological split, where later courses are new, the model scored 24.79% against 16.10% for
the last topic (measured when `max_len` still counted topics, not videos). The patterns learn how topics follow each other inside courses, so most of the model's
advantage applies to courses seen in training.

## Study path planner (`planner.html`)

A second single-file page, mined with the same settings but on the original Chinese topic names in
`concept-video backup.txt` rather than their machine translation, which merges distinct topics and garbles
many names. The discipline of each row is taken from the English file, which has the same rows in the same
order. After mining, the 9,793 Chinese names the page offers were translated by hand (by Claude) into
`translations/topics_en.tsv`, and `planner` applies them when it writes the page; `mined/planner/untranslated.tsv`
lists any page topic still without an English name. Where two Chinese synonyms got the same English name in
one discipline (307 pairs, such as 二叉搜索树 and 二叉查找树), the page adds the Chinese in brackets. Each use
has its own tab:

- **My path**: with a goal, the route from what you've learned to the goal, which is the chain "Explore" shows
  for it. With no goal it asks you to pick one. Tick a step when you've learned it and the rest is replanned.
- **Learned**: every topic, one discipline at a time and A to Z, to tick as learned; there is no limit and no
  order, and the ticks are kept in the browser. A search box narrows the list.
- **Explore**: every topic within 6 steps of what you've learned, one discipline and one step count at a time,
  10 per page. A search box lists the reachable topics whose name matches, one discipline at a time
  with every step count together, each marked with its steps away.
- **Where to start**: for a discipline, the topics learners begin it with, each with the chain that follows.

How chains are made ("My path" and "Explore"):

- **Prediction search**: each learned topic is a start of its own. (Scored together as one itemset,
  learned topics from different courses split each step's share between hundreds of predicted topics, and
  almost no step passes the cutoff.) A breadth-first search expands each reached topic once: the prediction
  function scores the topic's own context (its start topic, then each step of its chain as its own video,
  the last 3 videos), and every topic the rules predict that the search has not reached yet becomes a next
  step if its share is at least 1% (`MIN_LINK`). The search runs once each time the learned topics change;
  both tabs read its result until the next change.
- **Share**: a step's score over the scores of every topic predicted from the same context. Learned topics
  are never steps.
- **One rule for every chain**: the fewest steps, at most 6, and of the chains with that many steps, the one
  whose shares multiply to most. The chain "Explore" shows for a topic is the route you get when you set it
  as the goal.
- **Evidence**: each route step shows its share and the context it was predicted from.

"Where to start" still reads 2-video links: a link goes from `x` to `t` if `t` came after `x` in at least 1%
of the sessions with `x` (`MIN_LINK` in the page; `MIN_LINK` in `src/model.rs`, used by the scored path, is
still 5%). **Entry topics** are topics of a discipline that at most one topic of
the same discipline links to, and that link straight to at least 3 of its topics, the most first. Each shows
its strongest 3-step chain of links inside the discipline.

The page no longer shows the model's own predicted path. `planner` still scores that path on the test set,
since it measures how far ahead the patterns predict well:

At each of the 75,822 points, every topic watched so far
in the session is taken as the learned set and a 6-step path is planned; a step is a hit if its topic is
watched later in that session. The baseline plans the most common topics not yet watched. Goal routes are not
scored: sessions do not record a goal.

| Step | Path reaches this step | Hit when reached | Hit, of all points | Baseline hit |
|---|---|---|---|---|
| 1 | 95.97% | 65.05% | 62.43% | 8.04% |
| 2 | 90.76% | 34.13% | 30.97% | 3.44% |
| 3 | 81.51% | 20.72% | 16.89% | 2.62% |
| 4 | 74.47% | 15.02% | 11.18% | 2.16% |
| 5 | 66.45% | 11.52% | 7.65% | 4.69% |
| 6 | 64.90% | 8.88% | 5.77% | 3.61% |

Each step is roughly half as reliable as the one before, so the page draws route steps from the fourth on as
rougher guesses. Requirements for the page are in `PLANNER_UI.md`.

## Pipeline

- **Data**: [MOOCCubeX](https://github.com/THU-KEG/MOOCCubeX). `time, user, video` come from the watch segments in `user-video.json`.
- **Topics**: each video's 3 most specific linked topics, as an itemset. Topics are dropped if they have a
  one-character name, name a person or institution, or link to fewer than 5 videos. Names come from automatic
  extraction, so some are truncated or noisy.
- **Cleaning**: keep calendar 2020 (other timestamps are bad client clocks) and drop sessions of more than 100 videos (automated traffic).
- **Split**: 70/15/15 by user, so no user is in two sets.
- **Sessions**: per user, cut at gaps over 20 minutes. Repeated segments of one video count as one video.
- **Mining**: PrefixSpan with itemsets; all frequent subsequences, not only maximal ones. Length counts
  elements (videos), not items: at `max_len=3` a pattern spans up to 3 videos with up to 3 topics each.
- **Prediction**: each pattern `<prefix, t>` whose prefix ends at the last video gives
  `P(t | prefix) = support(<prefix, t>) / support(<prefix>)`, weighted by `4^(topics in prefix)`; a topic scores the sum of its rules.
- **Score**: Hit@1 over all (query, next video) pairs; a hit means the predicted topic is one of the next video's topics.

Settings were chosen on the validation set and live in `CHOSEN` in `src/main.rs`: `min_support=10, max_len=3`.
Max length is 3, not 7, because long patterns are only affordable at high support, which lowers the score.
These settings scored 68.93% on validation when `max_len` still counted topics; validation has not been rerun since.

## Audit

Each run writes what it calculates to `mined/evaluate/`, `mined/tune/` or `mined/planner/` at the moment it is calculated, replacing
the previous run of that kind, so a crash still leaves every earlier step. Topics appear as the ids in `topics.tsv`.

| File | Contents |
|---|---|
| `log.tsv` | Config, row counts and hit rates, in calculation order |
| `topics.tsv` | Every usable topic with its links; the id is empty if no video kept it |
| `video_topics.tsv` | Each video's topics |
| `sessions_<set>.tsv` | Each session, one row per video |
| `patterns.tsv` | Every mined pattern, with its support and its prefix's support |
| `topic_views.tsv`, `topic_sessions.tsv` | The counts behind the two baselines |
| `questions_<set>.tsv` | Each distinct query, its pair count and its ground truth counts |
| `predictions_<model>.tsv` | Each question's prediction and hits |
| `tune_results.tsv` | `tune` only: the hit rate of each config |

## Guardrails

Mining stops cleanly at 10M patterns or 12 GB of resident memory (`Limits` in `src/prefixspan.rs`). `tune` then
doubles the support and retries; `evaluate` fails instead, so it never scores a different model than `CHOSEN`.

## Reproduce

Put these files from `https://lfs.aminer.cn/misc/moocdata/data/mooccube2/` into `data/mooc/`:
`relations/user-video.json`, `relations/video_id-ccid.txt`, `relations/concept-video.txt`.

```
cargo test --release
cargo run --release -- tune 10 3    # validation set only; writes mined/tune/
cargo run --release -- evaluate     # test set; writes mined/evaluate/, demo.html and target/selftest.html
cargo run --release -- planner      # Chinese topic names; test set; writes mined/planner/ and planner.html, leaves demo.html alone
```

Opening `target/selftest.html` recomputes every distinct test query in the browser and compares it with the Rust model.
