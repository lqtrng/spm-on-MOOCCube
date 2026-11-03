//! Writes the single-file demo: the mined pattern tree and the config, embedded in `demo.html`.

use crate::data::Topic;
use crate::model::{Agg, Config, Model, ProbabilityModel};
use crate::prefixspan::Patterns;
use serde_json::{Value, json};

const TEMPLATE: &str = include_str!("demo.html");

pub fn demo_html(patterns: &Patterns, cfg: &Config, topic_names: &[String]) -> String {
    let n = &patterns.nodes;
    // Only topics in some pattern can influence a prediction, so only those are offered.
    let mut names = serde_json::Map::new();
    for c in patterns.children(&n[0]) {
        let topic = &topic_names[c.item as usize];
        let body = topic.strip_prefix("K_").unwrap_or(topic);
        let (name, field) = body.rsplit_once('_').unwrap_or((body, ""));
        names.insert(c.item.to_string(), json!({ "name": name, "field": field }));
    }
    let model = json!({
        // Nodes are in breadth-first order, so each node's first child follows from the counts.
        "key": n.iter().map(|x| 2 * x.item as u32 + x.is_seq as u32).collect::<Vec<_>>(),
        "support": n.iter().map(|x| x.support).collect::<Vec<_>>(),
        "n": n.iter().map(|x| x.n_children).collect::<Vec<_>>(),
        "names": names,
        "config": {
            "min_support": cfg.min_support,
            "max_len": cfg.max_len,
            "agg": if cfg.agg == Agg::Sum { "Sum" } else { "Max" },
            "alpha": cfg.alpha,
            "len_weight": cfg.len_weight,
            "recency": cfg.recency,
        },
    });
    // JSON inside <script>: only "</" could end the element early.
    TEMPLATE.replace("/*MODEL*/", &model.to_string().replace("</", "<\\/"))
}

/// The demo plus a script that recomputes every given query in the browser and compares it with
/// the Rust model: same prediction, same sum and maximum of scores. Prints its verdict into the page.
pub fn selftest_html(demo: &str, model: &ProbabilityModel, queries: &[Vec<Vec<Topic>>]) -> String {
    let cases: Vec<Value> = queries
        .iter()
        .map(|q| {
            let s = model.scores(q);
            json!([q, model.predict(q), s.iter().sum::<f64>(), s.iter().cloned().fold(0.0, f64::max)])
        })
        .collect();
    let script = format!(
        r#"<script>
const CASES = {};
let bad = 0, maxDiff = 0, firstBad = "";
for (const [q, pred, sum, top] of CASES) {{
  const s = scoresOf(q);
  const p = predict(q);
  let a = 0, b = 0;
  for (const x of s) {{ a += x; b = Math.max(b, x); }}
  maxDiff = Math.max(maxDiff, Math.abs(a - sum), Math.abs(b - top));
  if (p !== pred) {{ bad++; if (!firstBad) firstBad = JSON.stringify(q) + " js=" + p + " rust=" + pred; }}
}}
const out = document.createElement("pre");
out.id = "selftest";
out.style.cssText = "position:fixed;left:0;bottom:0;margin:0;padding:8px;background:#000;color:#fff";
out.textContent = `SELFTEST cases=${{CASES.length}} mismatches=${{bad}} max_score_diff=${{maxDiff}} ${{firstBad}}`;
document.body.append(out);
</script>"#,
        Value::Array(cases)
    );
    demo.replace("<!--SELFTEST-->", &script)
}
