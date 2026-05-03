use std::cmp::Ordering;
use std::collections::HashMap;

use crate::history::EvalRunRecord;
use crate::runner::{TaskOutcome, TaskStatus};

/// Output of comparing two runs.
#[derive(Debug, Clone, PartialEq)]
pub struct RegressionReport {
    /// Task ids that PASSED in `older` but FAILED in `newer`.
    pub regressed: Vec<String>,
    /// Task ids that FAILED in `older` but PASSED in `newer`.
    pub newly_fixed: Vec<String>,
    /// Task ids present in older but absent in newer.
    pub removed: Vec<String>,
    /// Task ids present in newer but absent in older.
    pub added: Vec<String>,
    /// Metadata attribution candidates, ranked most-likely-culprit first.
    pub attribution: Vec<AttributionCandidate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttributionCandidate {
    /// Field name (e.g. "config_hash", "model", "commit_sha").
    pub field: String,
    /// Older value as a string.
    pub older_value: String,
    /// Newer value as a string.
    pub newer_value: String,
    /// 0.0..=1.0 — heuristic score; higher = more suspicious.
    pub score: f32,
}

fn is_pass(outcome: &TaskOutcome) -> bool {
    matches!(outcome.status, TaskStatus::Pass)
}

pub struct RegressionBisector;

impl RegressionBisector {
    /// Compare two records. `older` is the baseline; `newer` is the candidate suspect.
    pub fn compare(older: &EvalRunRecord, newer: &EvalRunRecord) -> RegressionReport {
        let older_map: HashMap<&str, &TaskOutcome> =
            older.outcomes.iter().map(|o| (o.id.as_str(), o)).collect();
        let newer_map: HashMap<&str, &TaskOutcome> =
            newer.outcomes.iter().map(|o| (o.id.as_str(), o)).collect();

        let mut regressed = Vec::new();
        let mut newly_fixed = Vec::new();
        let mut removed = Vec::new();
        let mut added = Vec::new();

        // Check older outcomes against newer
        for (id, old_outcome) in &older_map {
            if let Some(new_outcome) = newer_map.get(id) {
                if is_pass(old_outcome) && !is_pass(new_outcome) {
                    regressed.push(id.to_string());
                } else if !is_pass(old_outcome) && is_pass(new_outcome) {
                    newly_fixed.push(id.to_string());
                }
            } else {
                removed.push(id.to_string());
            }
        }

        // Find added (in newer but not older)
        for id in newer_map.keys() {
            if !older_map.contains_key(id) {
                added.push(id.to_string());
            }
        }

        regressed.sort();
        newly_fixed.sort();
        removed.sort();
        added.sort();

        // Attribution
        let mut attribution = Vec::new();

        if older.config_hash != newer.config_hash {
            attribution.push(AttributionCandidate {
                field: "config_hash".into(),
                older_value: older.config_hash.clone(),
                newer_value: newer.config_hash.clone(),
                score: 0.9,
            });
        }

        if older.model != newer.model {
            attribution.push(AttributionCandidate {
                field: "model".into(),
                older_value: older.model.clone(),
                newer_value: newer.model.clone(),
                score: 0.8,
            });
        }

        if older.commit_sha != newer.commit_sha {
            let to_str = |o: &Option<String>| o.as_deref().unwrap_or("").to_string();
            attribution.push(AttributionCandidate {
                field: "commit_sha".into(),
                older_value: to_str(&older.commit_sha),
                newer_value: to_str(&newer.commit_sha),
                score: 0.5,
            });
        }

        if older.tag != newer.tag {
            let to_str = |o: &Option<String>| o.as_deref().unwrap_or("").to_string();
            attribution.push(AttributionCandidate {
                field: "tag".into(),
                older_value: to_str(&older.tag),
                newer_value: to_str(&newer.tag),
                score: 0.1,
            });
        }

        attribution.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.field.cmp(&b.field))
        });

        RegressionReport {
            regressed,
            newly_fixed,
            removed,
            added,
            attribution,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::EvalRunRecord;
    use crate::runner::{TaskOutcome, TaskStatus};
    use chrono::Utc;

    fn make_outcome(id: &str, status: TaskStatus) -> TaskOutcome {
        TaskOutcome {
            id: id.to_owned(),
            status,
            duration_ms: 0,
            tokens_input: 0,
            tokens_output: 0,
            cost_usd: 0.0,
            success_check_stdout: String::new(),
            success_check_stderr: String::new(),
            success_check_exit_code: None,
        }
    }

    fn make_record(
        id: &str,
        model: &str,
        config_hash: &str,
        commit_sha: Option<&str>,
        tag: Option<&str>,
        outcomes: Vec<TaskOutcome>,
    ) -> EvalRunRecord {
        EvalRunRecord {
            id: id.to_owned(),
            timestamp: Utc::now(),
            commit_sha: commit_sha.map(str::to_owned),
            model: model.to_owned(),
            config_hash: config_hash.to_owned(),
            tag: tag.map(str::to_owned),
            outcomes,
        }
    }

    #[test]
    fn compare_identical_runs_returns_no_diffs() {
        let outcomes = vec![
            make_outcome("T001", TaskStatus::Pass),
            make_outcome("T002", TaskStatus::Fail),
        ];
        let older = make_record("r1", "model-a", "hash-x", Some("sha1"), Some("nightly"), outcomes.clone());
        let newer = make_record("r2", "model-a", "hash-x", Some("sha1"), Some("nightly"), outcomes);

        let report = RegressionBisector::compare(&older, &newer);

        assert!(report.regressed.is_empty());
        assert!(report.newly_fixed.is_empty());
        assert!(report.added.is_empty());
        assert!(report.removed.is_empty());
        assert!(report.attribution.is_empty());
    }

    #[test]
    fn compare_detects_single_regression() {
        let older = make_record(
            "r1", "m", "h", None, None,
            vec![make_outcome("T001", TaskStatus::Pass)],
        );
        let newer = make_record(
            "r2", "m", "h", None, None,
            vec![make_outcome("T001", TaskStatus::Fail)],
        );

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.regressed, vec!["T001"]);
        assert!(report.newly_fixed.is_empty());
    }

    #[test]
    fn compare_detects_single_newly_fixed() {
        let older = make_record(
            "r1", "m", "h", None, None,
            vec![make_outcome("T001", TaskStatus::Fail)],
        );
        let newer = make_record(
            "r2", "m", "h", None, None,
            vec![make_outcome("T001", TaskStatus::Pass)],
        );

        let report = RegressionBisector::compare(&older, &newer);

        assert!(report.regressed.is_empty());
        assert_eq!(report.newly_fixed, vec!["T001"]);
    }

    #[test]
    fn compare_detects_added_and_removed_tasks() {
        let older = make_record(
            "r1", "m", "h", None, None,
            vec![
                make_outcome("T001", TaskStatus::Pass),
                make_outcome("T002", TaskStatus::Pass),
            ],
        );
        let newer = make_record(
            "r2", "m", "h", None, None,
            vec![
                make_outcome("T001", TaskStatus::Pass),
                make_outcome("T003", TaskStatus::Pass),
            ],
        );

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.removed, vec!["T002"]);
        assert_eq!(report.added, vec!["T003"]);
        assert!(report.regressed.is_empty());
        assert!(report.newly_fixed.is_empty());
    }

    #[test]
    fn compare_treats_timeout_as_fail() {
        let older = make_record(
            "r1", "m", "h", None, None,
            vec![make_outcome("T001", TaskStatus::Pass)],
        );
        let newer = make_record(
            "r2", "m", "h", None, None,
            vec![make_outcome("T001", TaskStatus::Timeout)],
        );

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.regressed, vec!["T001"]);
    }

    #[test]
    fn compare_treats_agent_error_as_fail() {
        let older = make_record(
            "r1", "m", "h", None, None,
            vec![make_outcome("T001", TaskStatus::Pass)],
        );
        let newer = make_record(
            "r2", "m", "h", None, None,
            vec![make_outcome("T001", TaskStatus::AgentError)],
        );

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.regressed, vec!["T001"]);
    }

    #[test]
    fn compare_returns_regressed_ids_sorted() {
        let older = make_record(
            "r1", "m", "h", None, None,
            vec![
                make_outcome("T003", TaskStatus::Pass),
                make_outcome("T001", TaskStatus::Pass),
                make_outcome("T002", TaskStatus::Pass),
            ],
        );
        let newer = make_record(
            "r2", "m", "h", None, None,
            vec![
                make_outcome("T003", TaskStatus::Fail),
                make_outcome("T001", TaskStatus::Fail),
                make_outcome("T002", TaskStatus::Fail),
            ],
        );

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.regressed, vec!["T001", "T002", "T003"]);
    }

    #[test]
    fn compare_no_metadata_changes_returns_empty_attribution() {
        // Same metadata, but a regression exists (flake scenario).
        let older = make_record(
            "r1", "model-a", "hash-x", Some("sha1"), Some("nightly"),
            vec![make_outcome("T001", TaskStatus::Pass)],
        );
        let newer = make_record(
            "r2", "model-a", "hash-x", Some("sha1"), Some("nightly"),
            vec![make_outcome("T001", TaskStatus::Fail)],
        );

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.regressed, vec!["T001"]);
        assert!(report.attribution.is_empty());
    }

    #[test]
    fn compare_config_hash_change_ranked_first() {
        let older = make_record("r1", "m", "hash-old", None, None, vec![]);
        let newer = make_record("r2", "m", "hash-new", None, None, vec![]);

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.attribution.len(), 1);
        assert_eq!(report.attribution[0].field, "config_hash");
        assert!((report.attribution[0].score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn compare_model_change_ranked_below_config_hash() {
        let older = make_record("r1", "model-a", "hash-old", None, None, vec![]);
        let newer = make_record("r2", "model-b", "hash-new", None, None, vec![]);

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.attribution.len(), 2);
        assert_eq!(report.attribution[0].field, "config_hash");
        assert_eq!(report.attribution[1].field, "model");
    }

    #[test]
    fn compare_attribution_includes_only_fields_that_differ() {
        // model + tag differ; commit_sha + config_hash are the same.
        let older = make_record("r1", "model-a", "hash-x", Some("sha1"), Some("nightly"), vec![]);
        let newer = make_record("r2", "model-b", "hash-x", Some("sha1"), Some("adhoc"), vec![]);

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.attribution.len(), 2);
        // model score=0.8 > tag score=0.1
        assert_eq!(report.attribution[0].field, "model");
        assert_eq!(report.attribution[1].field, "tag");
        // Confirm commit_sha and config_hash are absent
        assert!(!report.attribution.iter().any(|c| c.field == "commit_sha"));
        assert!(!report.attribution.iter().any(|c| c.field == "config_hash"));
    }

    #[test]
    fn compare_tag_change_alone_ranked_lowest() {
        let older = make_record("r1", "m", "h", None, Some("nightly"), vec![]);
        let newer = make_record("r2", "m", "h", None, Some("adhoc"), vec![]);

        let report = RegressionBisector::compare(&older, &newer);

        assert_eq!(report.attribution.len(), 1);
        assert_eq!(report.attribution[0].field, "tag");
        assert!((report.attribution[0].score - 0.1).abs() < f32::EPSILON);
    }
}
