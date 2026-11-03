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

Each run writes what it calculates to `mined/evaluate/` or `mined/tune/` at the moment it is calculated, replacing
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
```

Opening `target/selftest.html` recomputes every distinct test query in the browser and compares it with the Rust model.
