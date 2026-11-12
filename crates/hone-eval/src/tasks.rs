//! The tasks file of `hone eval`: tasks, `[eval]`, `[profiles.*]`, and their
//! expansion into runs (F3, F4, F5). Pure functions: nothing here touches a model.

use std::collections::BTreeMap;
use std::path::PathBuf;

use hone_config::Config;
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{
    COMPACTOR_NAMES, ContextTable, Flags, GraduatedTable, MemoryTable, RECALLER_NAMES,
    SKELETON_NAMES, SOURCE_NAMES, SkeletonTable, StrategySelection,
};

use crate::config::{EvalConfigError, TaskSpec};

/// The built-in profile with no overrides.
pub const DEFAULT_PROFILE: &str = "default";

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEval {
    base_profile: Option<String>,
    repeats: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct RawFile {
    model: Option<String>,
    tasks: Vec<TaskSpec>,
    #[serde(default)]
    eval: RawEval,
    #[serde(default)]
    profiles: BTreeMap<String, toml::Value>,
}

/// A parsed tasks file.
#[derive(Debug, Clone)]
pub struct TasksFile {
    pub tasks: Vec<TaskSpec>,
    pub model: Option<String>,
    label: String,
    source_sha256: String,
    base: Option<String>,
    repeats: Option<u32>,
    profiles: BTreeMap<String, Config>,
    /// Whether the built-in `default` profile exists (not after `--ablate`,
    /// which keeps only the base and what it generates).
    builtin_default: bool,
}

#[derive(Debug, Default)]
pub struct ExpandOptions {
    /// Selected profiles; empty = all.
    pub profiles: Vec<String>,
    pub repeats: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRun {
    pub profile: String,
    pub task: String,
    pub repeat: u32,
}

fn invalid(msg: String) -> EvalConfigError {
    EvalConfigError::Invalid(msg)
}

fn context(cfg: &mut Config) -> &mut ContextTable {
    cfg.context.get_or_insert_with(ContextTable::default)
}

impl TasksFile {
    /// Parse and validate. `file` labels errors (the path).
    pub fn from_toml_str(file: &str, raw: &str) -> Result<TasksFile, EvalConfigError> {
        let parsed: RawFile =
            toml::from_str(raw).map_err(|e| EvalConfigError::Toml(format!("{file}: {e}")))?;
        let mut seen = std::collections::HashSet::new();
        for task in &parsed.tasks {
            if !seen.insert(task.id.clone()) {
                return Err(EvalConfigError::DuplicateId(task.id.clone()));
            }
        }
        let mut profiles = BTreeMap::new();
        for (name, value) in parsed.profiles {
            let cfg: Config = value
                .try_into()
                .map_err(|e| invalid(format!("{file}: profile `{name}`: {e}")))?;
            let mut only_tables = cfg.clone();
            only_tables.context = None;
            only_tables.memory = None;
            if only_tables != Config::default() {
                return Err(invalid(format!(
                    "{file}: profile `{name}`: only [context] and [memory] are allowed in a profile"
                )));
            }
            profiles.insert(name, cfg);
        }
        let tf = TasksFile {
            tasks: parsed.tasks,
            model: parsed.model,
            label: file.to_owned(),
            source_sha256: hone_proto::sha256_hex(raw.as_bytes()),
            base: parsed.eval.base_profile,
            repeats: parsed.eval.repeats,
            profiles,
            builtin_default: true,
        };
        let base = tf.base_profile();
        if !tf.has_profile(&base) {
            return Err(invalid(format!(
                "{file}: [eval] base_profile = \"{base}\" is not a defined profile"
            )));
        }
        // Names and ranges, with the same parser as config.toml.
        for name in tf.profile_names() {
            tf.profile_selection(&name, None)?;
        }
        Ok(tf)
    }

    fn has_profile(&self, name: &str) -> bool {
        self.profiles.contains_key(name) || (self.builtin_default && name == DEFAULT_PROFILE)
    }

    /// SHA-256 (hex) of the tasks file's text.
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    /// Every problem with the files the tasks name, relative to `base_dir` (the
    /// tasks file's directory): missing working directories, missing or malformed
    /// memory fixtures. Empty when all is well.
    pub fn check_files(&self, base_dir: &std::path::Path) -> Vec<String> {
        let mut errors = Vec::new();
        for task in &self.tasks {
            let dir = base_dir.join(&task.working_dir);
            if !dir.is_dir() {
                errors.push(format!(
                    "task `{}`: working_dir `{}` is not a directory",
                    task.id,
                    dir.display()
                ));
            }
            if let Some(f) = &task.memory_fixture {
                if let Err(e) = crate::fixture::load(&base_dir.join(f)) {
                    errors.push(format!("task `{}`: memory_fixture {e}", task.id));
                }
            }
        }
        errors
    }

    /// `[eval] base_profile`, default `default`.
    pub fn base_profile(&self) -> String {
        self.base
            .clone()
            .unwrap_or_else(|| DEFAULT_PROFILE.to_owned())
    }

    /// `[eval] repeats`, default 1.
    pub fn repeats(&self) -> u32 {
        self.repeats.unwrap_or(1).max(1)
    }

    /// The base profile first, then the others sorted by name.
    pub fn profile_names(&self) -> Vec<String> {
        let base = self.base_profile();
        let mut rest: Vec<String> = self
            .profiles
            .keys()
            .filter(|k| **k != base)
            .cloned()
            .collect();
        if self.builtin_default
            && base != DEFAULT_PROFILE
            && !self.profiles.contains_key(DEFAULT_PROFILE)
        {
            rest.push(DEFAULT_PROFILE.to_owned());
        }
        rest.sort();
        let mut names = vec![base];
        names.extend(rest);
        names
    }

    /// The profile applied over `user` like a project file would be, except that
    /// it may also widen. No environment variable or flag takes part.
    pub fn profile_selection(
        &self,
        name: &str,
        user: Option<&ConfigLayer>,
    ) -> Result<StrategySelection, EvalConfigError> {
        let cfg = match self.profiles.get(name) {
            Some(c) => c.clone(),
            None if self.has_profile(name) => Config::default(),
            None => {
                return Err(invalid(format!("{}: unknown profile `{name}`", self.label)));
            }
        };
        let layer = ConfigLayer {
            path: PathBuf::from(format!("{} [profiles.{name}]", self.label)),
            config: cfg,
        };
        StrategySelection::from_sources(user, None, &|_| None, &Flags::default())
            .and_then(|sel| sel.with_trusted_layer(&layer))
            .map_err(|e| invalid(e.to_string()))
    }

    /// Runs in order: each task, each repeat (1-based), each selected profile.
    pub fn expand(&self, opts: &ExpandOptions) -> Result<Vec<PlannedRun>, EvalConfigError> {
        let all = self.profile_names();
        for p in &opts.profiles {
            if !all.contains(p) {
                return Err(invalid(format!(
                    "{}: unknown profile `{p}` (profiles: {})",
                    self.label,
                    all.join(", ")
                )));
            }
        }
        let chosen: Vec<&String> = all
            .iter()
            .filter(|n| opts.profiles.is_empty() || opts.profiles.contains(n))
            .collect();
        let repeats = opts.repeats.unwrap_or_else(|| self.repeats()).max(1);
        let mut runs = Vec::new();
        for task in &self.tasks {
            for repeat in 1..=repeats {
                for profile in &chosen {
                    runs.push(PlannedRun {
                        profile: (*profile).clone(),
                        task: task.id.clone(),
                        repeat,
                    });
                }
            }
        }
        Ok(runs)
    }

    /// `--ablate SLOT [base]`: a tasks file whose profiles are the base and one
    /// generated profile per built-in value of `SLOT` other than the base's.
    pub fn with_ablation(
        &self,
        slot: &str,
        base: Option<&str>,
    ) -> Result<TasksFile, EvalConfigError> {
        let base_name = base
            .map(str::to_owned)
            .unwrap_or_else(|| self.base_profile());
        if !self.has_profile(&base_name) {
            return Err(invalid(format!(
                "{}: unknown base profile `{base_name}`",
                self.label
            )));
        }
        let sel = self.profile_selection(&base_name, None)?;
        let base_cfg = self.profiles.get(&base_name).cloned().unwrap_or_default();

        // (profile name suffix, the config with the slot varied)
        let mut variants: Vec<(String, Config)> = Vec::new();
        let mut vary = |value: &str, f: &dyn Fn(&mut Config)| {
            let mut cfg = base_cfg.clone();
            f(&mut cfg);
            variants.push((format!("{slot}={value}"), cfg));
        };
        match slot {
            "context.skeleton" => {
                for v in SKELETON_NAMES.iter().filter(|v| **v != sel.skeleton.value) {
                    vary(v, &|cfg| {
                        context(cfg)
                            .skeleton
                            .get_or_insert_with(SkeletonTable::default)
                            .use_ = Some((*v).to_owned());
                    });
                }
            }
            "context.compactor" => {
                for v in COMPACTOR_NAMES
                    .iter()
                    .filter(|v| **v != sel.compactor.value)
                {
                    vary(v, &|cfg| context(cfg).compactor = Some((*v).to_owned()));
                }
            }
            "context.graduated.phase" => {
                for v in [false, true] {
                    if v != sel.graduated_phase.value {
                        vary(&v.to_string(), &|cfg| {
                            context(cfg)
                                .graduated
                                .get_or_insert_with(GraduatedTable::default)
                                .phase = Some(v);
                        });
                    }
                }
            }
            "memory.recaller" => {
                for v in RECALLER_NAMES.iter().filter(|v| **v != sel.recaller.value) {
                    vary(v, &|cfg| {
                        cfg.memory.get_or_insert_with(MemoryTable::default).recaller =
                            Some((*v).to_owned());
                    });
                }
            }
            s if s.starts_with("context.sources:") => {
                let source = &s["context.sources:".len()..];
                if !SOURCE_NAMES.contains(&source) {
                    return Err(invalid(format!(
                        "unknown source `{source}` in ablation slot `{slot}`; sources: {}",
                        SOURCE_NAMES.join(", ")
                    )));
                }
                if sel.sources.value.iter().any(|n| n == source) {
                    let kept: Vec<String> = sel
                        .sources
                        .value
                        .iter()
                        .filter(|n| *n != source)
                        .cloned()
                        .collect();
                    vary("off", &|cfg| context(cfg).sources = Some(kept.clone()));
                }
            }
            _ => {
                return Err(invalid(format!(
                    "unknown ablation slot `{slot}`; slots: context.skeleton, context.compactor, \
                     context.graduated.phase, memory.recaller, context.sources:<name>"
                )));
            }
        }

        let mut profiles = BTreeMap::new();
        profiles.insert(base_name.clone(), base_cfg);
        profiles.extend(variants);
        let out = TasksFile {
            tasks: self.tasks.clone(),
            model: self.model.clone(),
            label: self.label.clone(),
            source_sha256: self.source_sha256.clone(),
            base: Some(base_name.clone()),
            repeats: self.repeats,
            profiles,
            builtin_default: base_name == DEFAULT_PROFILE,
        };
        for name in out.profile_names() {
            out.profile_selection(&name, None)?;
        }
        Ok(out)
    }
}
