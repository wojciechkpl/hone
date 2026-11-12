//! `hone strategies [--json] [--check]`: which context and memory strategies
//! are active, with their parameters, what chose each parameter and where each
//! strategy is registered from.

use hone_config::strategy::StrategySelection;
use hone_core::strategy::Origin as RegistryOrigin;
use serde_json::{Map, Value, json};

/// One slot of the report.
struct Slot {
    slot: &'static str,
    name: Value,
    params: Value,
    /// What chose the strategy (or, for `context.sources`, the list).
    origin: String,
    /// Per parameter, what chose it.
    param_origins: Map<String, Value>,
}

/// An `f32` as the `f64` of its shortest decimal form (`0.4` stays `0.4`).
fn float(v: f32) -> Value {
    format!("{v}")
        .parse::<f64>()
        .map_or(Value::Null, |f| json!(f))
}

fn registry_label(o: &RegistryOrigin) -> String {
    match o {
        RegistryOrigin::Builtin => "builtin".to_owned(),
        RegistryOrigin::Feature(n) => format!("feature:{n}"),
        RegistryOrigin::Plugin(n) => format!("plugin:{n}"),
    }
}

/// Where `name` is registered from, for the slot `key`; a list gives the first
/// origin that is not built-in, else `builtin`.
fn registry_origin(
    registry: &[(String, String, RegistryOrigin)],
    key: &str,
    name: &Value,
) -> String {
    let names: Vec<&str> = match name {
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        other => other.as_str().into_iter().collect(),
    };
    let mut found = Vec::new();
    for n in names {
        if let Some((_, _, o)) = registry.iter().find(|(s, rn, _)| s == key && rn == n) {
            found.push(o.clone());
        }
    }
    found
        .iter()
        .find(|o| **o != RegistryOrigin::Builtin)
        .or_else(|| found.first())
        .map_or_else(|| "unknown".to_owned(), registry_label)
}

fn origins(pairs: &[(&str, String)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, label)| ((*k).to_owned(), Value::String(label.clone())))
        .collect()
}

fn slots(sel: &StrategySelection) -> Vec<Slot> {
    vec![
        Slot {
            slot: "context.sources",
            name: json!(sel.sources.value),
            // Parameters of the sources that have any, keyed by source name.
            params: if sel.sources.value.iter().any(|n| n == "env-snapshot") {
                json!({ "env-snapshot": { "timeout_secs": sel.env_snapshot_timeout_secs.value } })
            } else {
                json!({})
            },
            origin: sel.sources.origin.label(),
            param_origins: Map::new(),
        },
        Slot {
            slot: "context.skeleton",
            name: json!(sel.skeleton.value),
            params: json!({
                "max_tokens": sel.skeleton_max_tokens.value,
                "gap_threshold": float(sel.skeleton_gap_threshold.value),
            }),
            origin: sel.skeleton.origin.label(),
            param_origins: origins(&[
                ("max_tokens", sel.skeleton_max_tokens.origin.label()),
                ("gap_threshold", sel.skeleton_gap_threshold.origin.label()),
            ]),
        },
        Slot {
            slot: "context.compactor",
            name: json!(sel.compactor.value),
            params: json!({
                "phase": sel.graduated_phase.value,
                "budget_ratio": sel.budget_ratio.value.map_or(Value::Null, float),
                "budget_cap": sel.budget_cap.value.map_or(Value::Null, float),
            }),
            origin: sel.compactor.origin.label(),
            param_origins: origins(&[
                ("phase", sel.graduated_phase.origin.label()),
                ("budget_ratio", sel.budget_ratio.origin.label()),
                ("budget_cap", sel.budget_cap.origin.label()),
            ]),
        },
        Slot {
            slot: "memory.recaller",
            name: json!(sel.recaller.value),
            params: json!({
                "top_k": sel.top_k.value,
                "exclude_kinds": sel.exclude_kinds.value,
            }),
            origin: sel.recaller.origin.label(),
            param_origins: origins(&[
                ("top_k", sel.top_k.origin.label()),
                ("exclude_kinds", sel.exclude_kinds.origin.label()),
            ]),
        },
        Slot {
            slot: "memory.writer",
            name: json!(sel.writer.value),
            params: json!({}),
            origin: sel.writer.origin.label(),
            param_origins: Map::new(),
        },
    ]
}

/// The report as one JSON object: `{ "slots": [...], "warnings": [...] }`.
pub fn render_json(
    sel: &StrategySelection,
    registry: &[(String, String, RegistryOrigin)],
) -> String {
    let slots: Vec<Value> = slots(sel)
        .into_iter()
        .map(|s| {
            json!({
                "slot": s.slot,
                "name": s.name,
                "params": s.params,
                "origin": s.origin,
                "registry_origin": registry_origin(registry, s.slot, &s.name),
                "param_origins": s.param_origins,
            })
        })
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
pub fn render_text(
    sel: &StrategySelection,
    registry: &[(String, String, RegistryOrigin)],
) -> String {
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
        out.push_str(&format!(
            "{}\n  active:  {name} ({})\n",
            s.slot,
            registry_origin(registry, s.slot, &s.name)
        ));
        if s.params != json!({}) {
            out.push_str(&format!("  params:  {}\n", s.params));
        }
        for (param, origin) in &s.param_origins {
            out.push_str(&format!(
                "    {param} chosen by: {}\n",
                origin.as_str().unwrap_or_default()
            ));
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
