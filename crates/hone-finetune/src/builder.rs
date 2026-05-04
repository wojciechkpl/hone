use std::path::Path;

use crate::example::TrainingExample;

#[derive(Debug, Default)]
pub struct DatasetBuilder {
    examples: Vec<TrainingExample>,
}

impl DatasetBuilder {
    pub fn new() -> Self {
        Self {
            examples: Vec::new(),
        }
    }

    pub fn add(&mut self, example: TrainingExample) -> &mut Self {
        self.examples.push(example);
        self
    }

    pub fn extend(&mut self, examples: impl IntoIterator<Item = TrainingExample>) -> &mut Self {
        self.examples.extend(examples);
        self
    }

    pub fn len(&self) -> usize {
        self.examples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.examples.is_empty()
    }

    pub fn examples(&self) -> &[TrainingExample] {
        &self.examples
    }

    /// Apply `filter_by_score(min)` in place. Returns the number of examples retained.
    pub fn retain_above_score(&mut self, min: f32) -> usize {
        let taken = std::mem::take(&mut self.examples);
        self.examples = crate::filter::filter_by_score(taken, min);
        self.examples.len()
    }

    /// Apply `dedup_by_prompt` in place. Returns the count removed.
    pub fn dedup(&mut self) -> usize {
        let taken = std::mem::take(&mut self.examples);
        let (kept, removed) = crate::filter::dedup_by_prompt(taken);
        self.examples = kept;
        removed
    }

    /// Apply `shuffle_with_seed(seed)` in place.
    pub fn shuffle(&mut self, seed: u64) {
        let taken = std::mem::take(&mut self.examples);
        self.examples = crate::filter::shuffle_with_seed(taken, seed);
    }

    /// Write the current examples to `path` as JSONL (one example per line).
    /// Creates parent dirs.
    pub fn write_jsonl(&self, path: &Path) -> Result<(), BuilderError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut file = std::fs::File::create(path)?;
        crate::jsonl::write_jsonl(&self.examples, &mut file)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialise: {0}")]
    Serialise(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::read_jsonl;
    use std::fs;

    fn ex(prompt: &str, response: &str) -> TrainingExample {
        TrainingExample::new(prompt, response)
    }

    fn ex_scored(prompt: &str, response: &str, score: f32) -> TrainingExample {
        TrainingExample::new(prompt, response).with_score(score)
    }

    #[test]
    fn new_is_empty() {
        let b = DatasetBuilder::new();
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn add_increments_len() {
        let mut b = DatasetBuilder::new();
        b.add(ex("p1", "r1"));
        assert_eq!(b.len(), 1);
        b.add(ex("p2", "r2"));
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn extend_appends_all() {
        let mut b = DatasetBuilder::new();
        b.extend(vec![ex("p1", "r1"), ex("p2", "r2"), ex("p3", "r3")]);
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn retain_above_score_drops_unscored() {
        let mut b = DatasetBuilder::new();
        b.add(ex("unscored", "r"));
        b.add(ex_scored("scored", "r", 0.8));
        let kept = b.retain_above_score(0.5);
        assert_eq!(kept, 1);
        assert_eq!(b.len(), 1);
        assert_eq!(b.examples()[0].prompt, "scored");
    }

    #[test]
    fn retain_above_score_drops_below_threshold() {
        let mut b = DatasetBuilder::new();
        b.add(ex_scored("low", "r", 0.3));
        b.add(ex_scored("high", "r", 0.9));
        b.retain_above_score(0.5);
        assert_eq!(b.len(), 1);
        assert_eq!(b.examples()[0].prompt, "high");
    }

    #[test]
    fn retain_above_score_returns_kept_count() {
        let mut b = DatasetBuilder::new();
        b.extend(vec![
            ex_scored("a", "r", 0.9),
            ex_scored("b", "r", 0.4),
            ex("c", "r"),
            ex_scored("d", "r", 0.7),
        ]);
        let kept = b.retain_above_score(0.6);
        assert_eq!(kept, 2);
    }

    #[test]
    fn dedup_removes_duplicates_keeps_first() {
        let mut b = DatasetBuilder::new();
        b.add(ex("same-prompt", "first-response"));
        b.add(ex("same-prompt", "second-response"));
        b.add(ex("unique", "r"));
        b.dedup();
        assert_eq!(b.len(), 2);
        assert_eq!(b.examples()[0].response, "first-response");
    }

    #[test]
    fn dedup_returns_removed_count() {
        let mut b = DatasetBuilder::new();
        b.extend(vec![
            ex("dup", "r1"),
            ex("dup", "r2"),
            ex("unique", "r"),
            ex("dup", "r3"),
        ]);
        let removed = b.dedup();
        assert_eq!(removed, 2);
    }

    #[test]
    fn shuffle_is_deterministic_for_same_seed() {
        let examples: Vec<_> = (0..10).map(|i| ex(&format!("p{i}"), "r")).collect();
        let mut b1 = DatasetBuilder::new();
        b1.extend(examples.clone());
        b1.shuffle(42);

        let mut b2 = DatasetBuilder::new();
        b2.extend(examples);
        b2.shuffle(42);

        assert_eq!(b1.examples(), b2.examples());
    }

    #[test]
    fn shuffle_changes_order_with_different_seed() {
        // 10 examples; probability of same order by chance with two distinct
        // LCG seeds is astronomically low.
        let examples: Vec<_> = (0..10).map(|i| ex(&format!("p{i}"), "r")).collect();
        let original_prompts: Vec<_> = examples.iter().map(|e| e.prompt.clone()).collect();

        let mut b = DatasetBuilder::new();
        b.extend(examples);
        b.shuffle(99);

        let shuffled_prompts: Vec<_> = b.examples().iter().map(|e| e.prompt.clone()).collect();
        assert_ne!(
            original_prompts, shuffled_prompts,
            "shuffle with seed 99 must reorder examples"
        );
    }

    #[test]
    fn write_jsonl_to_tempfile_round_trips_via_read_jsonl() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dataset.jsonl");

        let mut b = DatasetBuilder::new();
        b.add(ex_scored("q1", "a1", 0.9).with_metadata("model", "llama3"));
        b.add(ex("q2", "a2"));
        b.write_jsonl(&path).expect("write_jsonl");

        let content = fs::read_to_string(&path).expect("read file");
        // Must have two non-empty lines
        let lines: Vec<_> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2);

        let file = fs::File::open(&path).expect("open file");
        let parsed = read_jsonl(file).expect("read_jsonl");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].prompt, "q1");
        assert_eq!(parsed[1].prompt, "q2");
    }
}
