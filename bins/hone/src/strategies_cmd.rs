//! `hone strategies [--json] [--check]`: which context and memory strategies
//! are active, with their parameters and what chose each.

use hone_config::strategy::{Chosen, StrategySelection};
use serde_json::{Value, json};

/// One slot of the report.
struct Slot {
    slot: &'static str,
    name: Value,
    params: Value,
    origin: String,
}

fn slots(sel: &StrategySelection) -> Vec<Slot> {
    fn origin<T>(c: &Chosen<T>) -> String {
        c.origin.label()
    }
    vec![
        Slot {
            slot: "context.sources",
            name: json!(sel.sources.value),
            params: json!({}),
            origin: origin(&sel.sources),
        },
        Slot {
            slot: "context.skeleton",
            name: json!(sel.skeleton.value),
            params: json!({
                "max_tokens": sel.skeleton_max_tokens.value,
                "gap_threshold": f64::from(sel.skeleton_gap_threshold.value),
            }),
            origin: origin(&sel.skeleton),
        },
        Slot {
            slot: "context.compactor",
            name: json!(sel.compactor.value),
            params: json!({
                "phase": sel.graduated_phase.value,
                "budget_ratio": sel.budget_ratio.value,
            }),
            origin: origin(&sel.compactor),
        },
        Slot {
            slot: "memory.recaller",
            name: json!(sel.recaller.value),
            params: json!({
                "top_k": sel.top_k.value,
                "exclude_kinds": sel.exclude_kinds.value,
            }),
            origin: origin(&sel.recaller),
        },
        Slot {
            slot: "memory.writer",
            name: json!(sel.writer.value),
            params: json!({}),
            origin: origin(&sel.writer),
        },
    ]
}

/// The report as one JSON object: `{ "slots": [...], "warnings": [...] }`.
pub fn render_json(sel: &StrategySelection) -> String {
    let slots: Vec<Value> = slots(sel)
        .into_iter()
        .map(|s| json!({ "slot": s.slot, "name": s.name, "params": s.params, "origin": s.origin }))
        .collect();
    let warnings: Vec<Value> = sel
        .warnings
        .iter()
        .map(|w| json!({ "path": w.path.display().to_string(), "message": w.message }))
        .collect();
    let out = json!({ "slots": slots, "warnings": warnings });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string())
    )
}

/// The report as text: one block per slot, then any warnings with their file.
pub fn render_text(sel: &StrategySelection) -> String {
    let mut out = String::new();
    for s in slots(sel) {
        let name = match &s.name {
            Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            other => other.as_str().unwrap_or_default().to_owned(),
        };
        out.push_str(&format!("{}\n  active:  {name}\n", s.slot));
        if s.params != json!({}) {
            out.push_str(&format!("  params:  {}\n", s.params));
        }
        out.push_str(&format!("  chosen by: {}\n\n", s.origin));
    }
    if !sel.warnings.is_empty() {
        out.push_str("warnings:\n");
        for w in &sel.warnings {
            out.push_str(&format!("  {}: {}\n", w.path.display(), w.message));
        }
    }
    out
}
