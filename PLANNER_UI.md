# Study path planner: UI/UX requirements

## Status

Built in `src/planner.html`, with one tab per use: My path, Learned, Explore, Where to start.
Changed from the requirements below at the user's request (2026-11-11):

- With no goal, My path asks for a goal instead of showing a likely path ahead (FR-20, second row).
- My path and Explore plan with the prediction function (2026-11-13): a breadth-first search with one
  start per learned topic, each reached topic expanded once by scoring its own chain as the context, every
  predicted topic not yet reached with a share of at least 1% added as a next step. Fewest steps first, then
  the largest product of step shares. The search runs once per change of the learned topics and is kept
  until the next. "Where to start" keeps its links, now at 1%. "Other first steps" was removed: a single
  search does not tell which other first steps still reach the goal as fast.
- Explore shows one discipline and one step count at a time, chosen with buttons.
- The Learned tab lists every topic by discipline, A to Z, to tick without searching.
- "What to learn first" (FR-32 to FR-35) was built and then removed: the user found no value in it.
- No page headline; the tab bar is the top of every page.

- Mined on the original Chinese topic names; the page's names are hand translations in
  `translations/topics_en.tsv` (2026-11-11).

Not built yet: export and import (FR-9), ticking several steps at once (FR-28), discipline markers beyond the
icons (VR-15), example starting topics on the empty page (VR-3).

## 1. Why rework

The page looks bad. `planner.html` reuses the screen of `demo.html`, which was drawn for one job: pick a few
topics, read one predicted topic. The planner's result is a path, and that screen has no place for it. The
rework is first a visual redesign (section 4), then the behaviour fixes that the new design needs (section 5).

**What looks wrong today**

| # | Problem |
|---|---|
| V1 | No identity: no title, no sentence saying what the page does, no starting point. It opens as a grid of grey boxes. |
| V2 | The result is not the hero. The path is squeezed into the narrower right column, while more than half the screen is a wall of 60 identical pill buttons. |
| V3 | The top-left pane is mostly empty space; the path pane next to it scrolls. Space goes where there is no content. |
| V4 | No hierarchy in the path: watched topics, steps, evidence and percentages are all small text in similar greys. Nothing draws the eye to "what do I study next". |
| V5 | The percentage floats at the far right edge, far from the topic it belongs to, with no label and no visual scale. |
| V6 | The evidence sentence ("252 of the 1276 sessions with … went on to this") is long, repeated under every step, and reads like a log line. |
| V7 | The alternatives are a dense table of up to 50 rows glued under the path. |
| V8 | Everything is one flat colour plus one accent. Disciplines, strong and weak steps, watched and proposed topics are not told apart. |
| V9 | It looks like a debugging tool: hard 1 px dividers, full-bleed panes, no margins, no rhythm in spacing. |
| V10 | Nothing moves or responds: a new path replaces the old one instantly, with no cue about what changed. |

**What behaves wrong today**

| # | Problem |
|---|---|
| P1 | The page is blank while the 62 MB model loads. |
| P2 | One picker serves two jobs through an "Add to watched / Set as goal" switch, which flips back by itself. |
| P3 | Order of watched topics changes the result, but topics cannot be reordered. |
| P4 | A 7th watched topic silently drops the oldest. Adding a topic twice silently does nothing. |
| P5 | The picker shows the first 60 matches with no count and no way to see the rest. |
| P6 | Topics that lead nowhere are hidden, so searching for one answers "No topic matches". |
| P7 | The percentage is never explained, and weak steps look the same as strong ones. |
| P8 | When the path picks up from an earlier watched topic, only a note under the path says so. |
| P9 | Marking the goal as watched silently clears the goal. |
| P10 | Nothing survives a reload. |

**What is added**

The planner stays the core. Three features are built around it, and the redesign must give each a place:

| Feature | In one sentence |
|---|---|
| Learned topics | The user ticks "learned" on topics the page shows, instead of typing a list; the page remembers them and plans from them. |
| What to learn first | For any topic, the topics that usually come before it. |
| Where to start | For a discipline, the topics to begin with and the main chains from them. |

## 2. Scope

**In scope:**

- The markup, styles and page code of `src/planner.html`.
- New planning code for the three added features and for an unlimited learned collection (section 5.9).

**Out of scope, do not change:**

- `demo.html` and `src/demo.html`.
- The scoring code shared with the demo (`scoresOf` and what it uses).
- `pathAhead` must stay the mirror of `ProbabilityModel::rollout` in `src/model.rs`: the Rust score in the
  README measures exactly that path.
- The Rust code, except to keep `planner_html` in `src/export.rs` working.

**Fixed constraints:**

- One self-contained `.html` file, opened from disk. No network requests, no external fonts, scripts or styles.
- Built by `cargo run --release -- planner` from the template, with its `/*MODEL*/` and `/*SCORING*/` placeholders kept.
- Works in light and dark themes, from 360 px wide to desktop, with keyboard only, and with a screen reader.
- The word shown to users is "topic".

## 3. Terms

| Term | Meaning |
|---|---|
| Learned topics | Every topic the user has ticked as learned. No size limit and no order: the planner reads them as one itemset, as if all were in one video. |
| Goal | One optional topic the user wants to reach. |
| Path | The ordered steps the planner proposes: "likely path ahead" without a goal, "route" with one. |
| Step | One proposed topic, with the pattern behind it: "N of the M sessions with A, then B went on to this". |
| Strength | N / M of that pattern, shown as a percentage. |
| Start topic | The learned topic the first step's evidence comes from, shown as the first stop of the path. |
| Dead-end topic | A topic no path can start from (5,195 of the 10,237). |
| Entry topic | A topic that leads on to others but rarely follows any: a place to begin a discipline. |

## 4. Design requirements

**Must** is required for the rework to be accepted. **Should** is expected unless it conflicts with a Must.

### 4.1 Character

- **VR-1 (Must)** The page reads as a route planner for studying, not as a data tool. A first-time visitor can
  tell within one screen what it is for and what to do first.
- **VR-2 (Must)** The page has a title, one sentence of explanation, and a clear first action.
- **VR-3 (Should)** The empty state offers a few example starting topics that produce a full path, so the user
  sees a result with one click.

### 4.2 Layout

- **VR-4 (Must)** The path is the largest and most prominent element on the page whenever one exists. On desktop
  it sits in the main column of a centred page with comfortable margins, not in a side pane.
- **VR-5 (Must)** Inputs are compact. Finding a topic and choosing a goal are search fields with
  suggestions that open on demand; no permanent wall of topic buttons.
- **VR-6 (Must)** The page scrolls as one document. No pane scrolls separately beside an empty one.
- **VR-7 (Must)** The visual order follows the user's order of thought: where I want to go (goal), how to get there
  (path), what I already know (learned), other options.
- **VR-8 (Must)** Content width is capped for readability; wide screens get margins, not stretched rows.

### 4.3 The path

- **VR-9 (Must)** The path is drawn as a route: a continuous line with a stop per topic, from the learned topics
  it starts at through each step to the goal. Learned stops, proposed steps and the goal are three visibly different kinds of stop.
- **VR-10 (Must)** Within a step the topic name is the dominant text. Discipline, strength and evidence are
  clearly secondary, in that order.
- **VR-11 (Must)** Strength sits next to the step it describes and is shown as a visual scale with its number,
  not as a bare percentage at the page edge.
- **VR-12 (Must)** The evidence sentence is short by default and expands on demand to the full counts.
- **VR-13 (Must)** The next step to study stands out from the later ones. Later, less reliable steps look
  progressively less certain (see FR-23), without becoming hard to read.
- **VR-14 (Must)** The goal stop looks like a destination. Reaching it is a visible moment.
- **VR-15 (Should)** Each discipline has a consistent small marker, so a path that changes discipline shows it
  at a glance.

### 4.4 Learned topics and the other features

- **VR-24 (Must)** The "learned" tick looks and sits the same on every topic the page shows, so the user learns
  it once. A learned topic is recognisable everywhere at a glance.
- **VR-25 (Must)** The learned collection can grow without limit and must not take over the page: it is
  summarised (a count, the most recent few) and opens on demand.
- **VR-26 (Must)** "What to learn first" and "Where to start" are reachable from the main screen in one action
  and never push the path out of first place while a path is shown.

### 4.5 Alternatives

- **VR-16 (Must)** Alternatives are secondary: collapsed or visually lighter than the path, showing a handful
  first with a way to see more.

### 4.6 Finish

- **VR-17 (Must)** A deliberate type scale with at least three clearly different levels (page title, step name,
  supporting text), and spacing taken from one consistent scale.
- **VR-18 (Must)** Groups are separated by space and surface, not by full-width hairlines.
- **VR-19 (Must)** Light and dark themes are both designed, with text contrast of at least 4.5:1.
- **VR-20 (Must)** Colour carries meaning consistently: one colour for the user's own topics, one for proposed
  steps, one for the goal. Strength is never shown by colour alone.
- **VR-21 (Should)** Changes are animated briefly: a replanned path shows which steps are new, and a step ticked
  as learned visibly joins the learned part of the route. Animations respect the reduced-motion setting.
- **VR-22 (Must)** Every interactive element has hover, focus, pressed and disabled looks.
- **VR-23 (Must)** Messages are written in plain words for a learner, with no internal terms such as "mined
  pattern", "support" or "session".

## 5. Functional requirements

### 5.1 Loading

- **FR-1 (Must)** Before the model is parsed, the page shows that it is loading. When parsing ends, the loading
  state is replaced by the right state of FR-20.
- **FR-2 (Must)** If the model is missing or cannot be parsed, the page says so in plain words instead of staying blank.

### 5.2 Learned topics

- **FR-3 (Must)** Every topic the page shows carries a "learned" tick: path steps, alternatives, search results,
  "what to learn first" and "where to start". One action ticks it, the same action unticks it.
- **FR-4 (Must)** The user never has to type a list of learned topics. Search exists to find a topic, not as the
  only way to record one.
- **FR-5 (Must)** The number of learned topics is unlimited. No topic is ever dropped to make room.
- **FR-6 (Must)** Learned topics are remembered across reloads on the same browser. If the browser refuses
  storage, the page still works for the visit and says progress will not be kept.
- **FR-7 (Must)** The user can review all learned topics, filter them by discipline, untick any, and clear all
  after a confirmation.
- **FR-8 (Should)** The last untick or clear can be undone in one action.
- **FR-9 (Should)** Learned topics can be exported to a file and imported from one, so progress can move to
  another browser.

### 5.3 Finding topics

- **FR-10 (Must)** Topics can be searched by name and filtered by discipline. Each result shows name,
  discipline, whether it is learned, and offers two actions: tick as learned, set as goal.
- **FR-11 (Must)** The result area shows how many topics match and lets the user reach all of them.
- **FR-12 (Must)** Dead-end topics appear in search and can be ticked as learned, since the user may really
  have learned them. They are marked "no path starts here".
- **FR-13 (Must)** The search field can be operated by keyboard alone: type, move through results, confirm, cancel.

### 5.4 Goal

- **FR-14 (Must)** Selecting a goal is enough to get a path. The user does nothing else.
- **FR-15 (Must)** The route is planned from the whole learned set and never passes through a topic already learned.
- **FR-16 (Must)** With no learned topic, or when nothing learned leads to the goal, the page says so and offers
  "what to learn first" for the goal in one action.
- **FR-17 (Must)** "Goals in reach" lists every topic a route can reach from the learned set, grouped by
  discipline. Inside a discipline topics go nearest first, then by the largest sum of link shares over a chain of
  that many steps; each shows that chain from a learned topic, and its sum. Any other topic can still be set as a goal from search (FR-16).
- **FR-18 (Must)** When the user ticks the goal itself as learned, the page says the goal is reached and offers
  to pick the next goal. It does not just clear the goal.
- **FR-19 (Must)** The goal can be cleared in one action. It is remembered across reloads like learned topics.

### 5.5 Path

- **FR-20 (Must)** Each state has its own message and a suggested next action:

  | State | What the user sees |
  |---|---|
  | Nothing learned, no goal | How to start: pick a goal, or browse where to start in a discipline |
  | Learned topics, no goal | "Likely path ahead" from the most recent learned topic that leads on |
  | Goal, route from a learned topic | The start topic, the steps, the goal |
  | Goal, nothing learned leads to it | That fact, and "see what to learn first" (FR-16) |
  | Goal too far | The reason, and the same offer |
  | Goal learned | That the goal is reached, and a prompt for the next goal |

- **FR-21 (Must)** Each step shows the topic name, its discipline, its strength, the evidence sentence, and the
  learned tick.
- **FR-22 (Must)** The page explains once, near the path, what the strength percentage means.
- **FR-23 (Must)** Steps are visibly graded by reliability. Measured on the test set, a step's topic is watched
  later in the session 66% of the time at step 1, 35% at step 2, 21% at step 3, and 9% at step 6. Steps 4 to 6
  must look less certain than steps 1 to 3, and the page must say why.
- **FR-24 (Must)** The path shows which learned topic it starts from. The other learned topics are not listed in
  the path.
- **FR-25 (Must)** The path updates on every tick, untick or goal change, with no submit button.
- **FR-26 (Should)** A change that replaces the path makes clear which steps are new.

### 5.6 Following the path

- **FR-27 (Must)** Ticking a step as learned replans the rest of the route from it.
- **FR-28 (Should)** The user can tick every step up to a chosen one in one action.
- **FR-29 (Must)** With a goal, the page shows progress toward it: steps done and steps left on the current route.
- **FR-30 (Must)** The alternatives to step 1 are listed with their strength and the learned tick.
- **FR-31 (Should)** With a goal set, each alternative shows whether the goal is still reachable after it, and in
  how many steps.

### 5.7 What to learn first

- **FR-32 (Must)** For any topic, one action opens "what to learn first": the topics that usually come before
  it, strongest first, each with the evidence "N of the M sessions with this topic had X earlier".
- **FR-33 (Must)** The view can be followed backwards: opening a listed topic shows what comes before that one.
- **FR-34 (Must)** Topics already learned are marked as done in this view; the others carry the learned tick and
  "set as goal".
- **FR-35 (Must)** A topic with nothing before it says so: it is a place to start.

### 5.8 Where to start

- **FR-36 (Must)** The user can choose a discipline and see its entry topics, each with how many topics it leads to.
- **FR-37 (Must)** For each entry topic the page can show the main chain that follows from it (the likely path ahead).
- **FR-38 (Must)** Every topic in this view carries the learned tick and "set as goal".
- **FR-39 (Should)** Each discipline shows how many of its topics the user has learned.

### 5.9 Planning rules behind the features

These change or add planning code. Each needs a check in the browser test the same way routes are checked today.

- **FR-40 (Must)** *Learned topics as one itemset.* The query is one video holding every learned topic; each
  chosen step is appended as a video of its own. The set has no size limit: the model only follows patterns
  that exist, whose itemsets hold 3 topics at most, so the subsets it tries stay bounded.
- **FR-41 (Must)** *Order does not matter.* The same learned topics ticked in any order give the same path.
- **FR-42 (Must)** *More never hurts.* Ticking a topic that leads nowhere does not remove or change a route.
- **FR-43 (Must)** *First step.* A step may be linked from any learned topic; later steps from the step before.
- **FR-44 (Must)** *Entry topics.* The topics of a discipline that lead on and that at most one other topic
  leads to, most studied first; a discipline with none falls back to its topics the fewest others lead to.
- **FR-45 (Must)** A plan after any single tick completes without a noticeable freeze with 500 learned topics.
  For reference, the slowest single route search measured in Chrome took about 50 ms.

### 5.10 Layout and access

- **FR-46 (Must)** At 360 px wide the order is: goal, path, learned summary, the other features. Nothing needs
  horizontal scrolling.
- **FR-47 (Must)** Long and odd topic names (the data has machine-translated names of up to 6 words) wrap without
  breaking the layout or hiding the strength.
- **FR-48 (Must)** Every action is reachable by keyboard, has a visible focus state, and has an accessible name.
  Path changes are announced to screen readers without stealing focus.
- **FR-49 (Must)** Strength, reliability and "learned" are never conveyed by colour alone.

## 6. Acceptance checks

Run these on the built `planner.html`, in light and dark themes, at 1280 px and 360 px.

1. Open the file with nothing stored: a loading state appears, then the start prompt.
2. Set a goal from search without ticking anything: the page asks for a learned topic and offers "what to learn
   first"; ticking a topic listed there makes the route appear.
3. Tick the first step of the route: it joins the learned topics and the route shows one step fewer.
4. Tick each remaining step: progress advances each time; ticking the goal shows that it is reached.
5. Reload: learned topics and goal are still there.
6. Open "where to start" for Computer Science: entry topics are listed. Tick one and set a topic on its chain as
   the goal: the route starts from the ticked topic.
7. Tick "human behavior" (Psychology), which cannot reach that goal: the route is unchanged.
8. Open "what to learn first" on the goal: the topics before it are listed, learned ones marked as done; follow
   one backwards.
9. With no goal, the likely path ahead is shown; ticking the same topics in another order gives the same path.
10. Search for a dead-end topic: it is listed, marked "no path starts here", and can be ticked.
11. Tick 30 topics: none is dropped, the learned summary stays compact, and the path still updates at once.
12. Untick a topic, then undo.
13. Repeat checks 2 to 10 with the keyboard only.
14. Show the page to someone who has not seen it: without help they can say what it is for, pick a goal, and
    read out the next topic to study.
15. Screenshot the page with a 4-step route at 1280 px: the path is the first thing the eye lands on, and no
    list of topic buttons is visible.
16. `demo.html` is byte-identical before and after, `cargo test --release` passes, and
    `cargo run --release -- planner` prints the path scores in the README.
