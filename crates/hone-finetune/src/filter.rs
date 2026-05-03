use std::collections::HashSet;

use crate::example::TrainingExample;

/// Keep only examples whose `score` is `Some(s) where s >= min`. Examples
/// with `score == None` are DROPPED (no score = no signal).
pub fn filter_by_score(examples: Vec<TrainingExample>, min: f32) -> Vec<TrainingExample> {
    examples
        .into_iter()
        .filter(|e| e.score.is_some_and(|s| s >= min))
        .collect()
}

/// Remove duplicates by `prompt` field. Keeps the FIRST occurrence; later
/// duplicates are dropped. Returns (deduped, removed_count).
pub fn dedup_by_prompt(examples: Vec<TrainingExample>) -> (Vec<TrainingExample>, usize) {
    let mut seen = HashSet::new();
    let mut kept = Vec::new();
    let mut removed = 0usize;
    for ex in examples {
        if seen.contains(&ex.prompt) {
            removed += 1;
        } else {
            seen.insert(ex.prompt.clone());
            kept.push(ex);
        }
    }
    (kept, removed)
}

/// Deterministic shuffle using a seeded LCG. Same seed -> same permutation.
/// Adds NO new dependencies (don't use rand).
///
/// Uses a Lehmer LCG with:
///   multiplier = 6364136223846793005
///   increment  = 1442695040888963407
/// (PCG-LCG step from Wikipedia). Apply via Fisher-Yates.
pub fn shuffle_with_seed(examples: Vec<TrainingExample>, seed: u64) -> Vec<TrainingExample> {
    let mut vec = examples;
    let n = vec.len();
    if n < 2 {
        return vec;
    }
    let mut state = seed;
    // Fisher-Yates from n-1 down to 1: advance LCG BEFORE the modulo.
    for i in (1..n).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let partner = (state as usize) % (i + 1);
        vec.swap(i, partner);
    }
    vec
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(prompt: &str, response: &str) -> TrainingExample {
        TrainingExample::new(prompt, response)
    }

    fn ex_scored(prompt: &str, response: &str, score: f32) -> TrainingExample {
        TrainingExample::new(prompt, response).with_score(score)
    }

    #[test]
    fn filter_by_score_drops_unscored() {
        let examples = vec![ex("unscored", "r"), ex_scored("scored", "r", 0.8)];
        let result = filter_by_score(examples, 0.5);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].prompt, "scored");
    }

    #[test]
    fn filter_by_score_drops_below_threshold() {
        let examples = vec![
            ex_scored("low", "r", 0.3),
            ex_scored("high", "r", 0.9),
            ex_scored("exact", "r", 0.5),
        ];
        let result = filter_by_score(examples, 0.5);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.score.unwrap_or(0.0) >= 0.5));
    }

    #[test]
    fn filter_by_score_keeps_at_threshold() {
        let examples = vec![ex_scored("at", "r", 0.5)];
        let result = filter_by_score(examples, 0.5);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].prompt, "at");
    }

    #[test]
    fn dedup_by_prompt_keeps_first_returns_count() {
        let examples = vec![
            ex("same-prompt", "first-response"),
            ex("same-prompt", "second-response"),
            ex("unique", "only-response"),
            ex("same-prompt", "third-response"),
        ];
        let (deduped, removed) = dedup_by_prompt(examples);
        assert_eq!(deduped.len(), 2);
        assert_eq!(removed, 2);
        // First occurrence of "same-prompt" must be kept.
        let same = deduped.iter().find(|e| e.prompt == "same-prompt").unwrap();
        assert_eq!(same.response, "first-response");
    }

    #[test]
    fn shuffle_with_seed_deterministic() {
        let examples: Vec<_> = (0..10).map(|i| ex(&format!("p{i}"), "r")).collect();
        let result_a = shuffle_with_seed(examples.clone(), 42);
        let result_b = shuffle_with_seed(examples, 42);
        assert_eq!(result_a, result_b);
    }
}
