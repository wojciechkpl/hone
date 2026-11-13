//! Shared fixtures for the G-row tests (`docs/plans/fts5-recall-spec.md`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;

use hone_memory::{Memory, MemoryKind, MemoryQuery};

pub fn mem(id: &str, content: &str, kind: MemoryKind, created_at: u64) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

pub fn q(text: &str, limit: usize, kind: Option<MemoryKind>) -> MemoryQuery {
    MemoryQuery {
        text: text.to_owned(),
        limit,
        kind,
    }
}

/// Word tokens as the spec defines them for G2 (letters, digits, `_`; lowercased; at
/// least 3 characters), WITHOUT stop-word removal.
pub fn word_tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| t.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect()
}

/// G3: 36 memories (10 topics x [winner, partial] + 16 noise) and 10 queries, each with
/// the id of its clear winner. Winners repeat the topic's terms; partials mention one.
pub fn corpus() -> Vec<Memory> {
    let topics: [(&str, &str, &str); 10] = [
        (
            "k8s",
            "To rotate the kubernetes ingress certificate, renew the certificate secret and restart the ingress controller. The kubernetes ingress certificate expires every ninety days.",
            "The kubernetes cluster upgrade is scheduled for the platform team next quarter.",
        ),
        (
            "pg",
            "Postgres autovacuum reclaims bloat; run vacuum manually when autovacuum falls behind on a bloated postgres table.",
            "The postgres replica lags during the nightly batch import.",
        ),
        (
            "rust",
            "The rust borrow checker reports a lifetime error when a reference outlives its owner; shorten the borrow or extend the lifetime.",
            "Rust edition migration notes for the workspace crates.",
        ),
        (
            "tf",
            "Terraform state lock stuck in dynamodb: release the terraform state lock with force-unlock after confirming no apply is running against the dynamodb table.",
            "Terraform modules live in the infrastructure repository.",
        ),
        (
            "graf",
            "Grafana dashboard shows latency percentile panels; the p99 latency percentile is computed per service on the grafana dashboard.",
            "Grafana alert routing was moved to the on-call schedule.",
        ),
        (
            "docker",
            "Docker layer cache misses slow builds; enable buildkit cache mounts so the docker layer cache survives between buildkit runs.",
            "Docker desktop licensing was reviewed by finance.",
        ),
        (
            "oauth",
            "The oauth refresh token expiry is one week; request a new access token with the refresh token before the oauth token expiry.",
            "OAuth consent screen wording was updated by legal.",
        ),
        (
            "redis",
            "Redis eviction policy: set maxmemory and the allkeys-lru eviction policy so redis evicts keys when maxmemory is reached.",
            "Redis cluster failover drill happens in March.",
        ),
        (
            "git",
            "Git interactive rebase lets you squash commits: run git rebase interactive, mark commits to squash, then force push the rebased branch.",
            "Git hooks are installed by the bootstrap script.",
        ),
        (
            "nginx",
            "Nginx proxy buffering and timeout: disable proxy buffering for streams and raise the nginx proxy read timeout for slow upstreams.",
            "Nginx access logs rotate weekly on the edge hosts.",
        ),
    ];
    let mut out = Vec::new();
    let mut t = 1_000u64;
    for (id, winner, partial) in topics {
        out.push(mem(&format!("{id}-win"), winner, MemoryKind::Feedback, t));
        out.push(mem(&format!("{id}-part"), partial, MemoryKind::Project, t + 1));
        t += 10;
    }
    let noise = [
        "Lunch menu: pasta and salad in the office kitchen on friday.",
        "Remember to water the plants near the window before the holiday.",
        "The quarterly offsite is booked at the lake house.",
        "Parking validation is available at the front desk.",
        "Coffee machine descaling is due every month.",
        "Team photo scheduled for tuesday afternoon.",
        "Bicycle storage moved to the basement garage.",
        "Birthday cake for the new hire arrives at noon.",
        "The printer on the third floor needs toner.",
        "Quiet hours in the library start at nine.",
        "Fire drill planned for the second week of june.",
        "New badge photos are taken in the lobby.",
        "Vending machine accepts cards and coins.",
        "Shuttle bus timetable changes in autumn.",
        "Recycling bins are collected on wednesday mornings.",
        "Standing desks can be requested from facilities.",
    ];
    for (i, n) in noise.iter().enumerate() {
        out.push(mem(&format!("noise-{i:02}"), n, MemoryKind::Conversation, 500 + i as u64));
    }
    assert_eq!(out.len(), 36);
    out
}

/// (query, id of the clear winner)
pub fn queries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("how do I rotate the kubernetes ingress certificate?", "k8s-win"),
        ("postgres autovacuum bloat keeps growing", "pg-win"),
        ("rust borrow checker lifetime error on a reference", "rust-win"),
        ("terraform state lock stuck in dynamodb", "tf-win"),
        ("grafana dashboard latency percentile panel", "graf-win"),
        ("docker layer cache buildkit misses", "docker-win"),
        ("oauth refresh token expiry", "oauth-win"),
        ("redis maxmemory eviction policy", "redis-win"),
        ("git interactive rebase squash commits", "git-win"),
        ("nginx proxy buffering timeout for streams", "nginx-win"),
    ]
}
