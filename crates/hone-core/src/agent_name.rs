//! Funky human-facing names for agents and sessions.
//!
//! Format: `{adj}-{noun}-{HHMM}-{XX}`
//!   - `{adj}-{noun}` from a 200-entry curated pool split by category
//!   - `{HHMM}` zero-padded UTC hour+minute derived from `seed_ns`
//!   - `{XX}` 2-character lowercase hex collision tiebreaker
//!
//! The seed is a nanosecond-resolution UNIX timestamp. Two calls in the same
//! minute land on different `XX` suffixes with high probability.

use std::sync::atomic::{AtomicU64, Ordering};

// ── Category ──────────────────────────────────────────────────────────────────

/// Logical agent role categories used to pick a themed name pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCategory {
    Generic      = 0,
    TestArchitect = 1,
    Reviewer     = 2,
    Implementer  = 3,
    Researcher   = 4,
    Debugger     = 5,
}

impl AgentCategory {
    /// Map an agent type string (e.g. `"test-architect"`, `"rust-expert"`) to
    /// a category. Unknown strings fall back to `Generic`.
    pub fn from_agent_type(s: &str) -> Self {
        match s {
            "test-architect" => Self::TestArchitect,
            "code-reviewer" | "code-reviewer-quick" => Self::Reviewer,
            "rust-expert"
            | "python-expert"
            | "flutter-expert"
            | "flutter-expert-engineer"
            | "ml-recommendations-expert" => Self::Implementer,
            "ai-researcher" | "general-purpose" => Self::Researcher,
            "debugger" | "performance-optimizer" => Self::Debugger,
            _ => Self::Generic,
        }
    }
}

// ── Name pools ────────────────────────────────────────────────────────────────
// Each entry satisfies `^[a-z]+-[a-z]+$`.
// Pools are sorted alphabetically for diff-friendly review.
// All 200 entries are unique across the union of all six pools.

/// Generic pool — 80 entries. Used for sessions, run IDs, and unknown types.
pub const GENERIC: &[&str] = &[
    "airy-comet",
    "amber-creek",
    "ample-canoe",
    "balmy-ember",
    "blithe-pebble",
    "bouncy-kettle",
    "breezy-comet",
    "bright-lantern",
    "brisk-cobalt",
    "bubbly-fern",
    "calm-harbor",
    "candid-mango",
    "cheerful-atlas",
    "chipper-prism",
    "cozy-beacon",
    "cozy-pebble",
    "crisp-nimbus",
    "dainty-flint",
    "dandy-crater",
    "dewy-marble",
    "drifty-anchor",
    "eager-delta",
    "easy-fossil",
    "fancy-pollen",
    "feisty-quartz",
    "fluffy-comet",
    "foamy-current",
    "friendly-cedar",
    "frosted-vale",
    "frothy-prism",
    "funky-gravel",
    "funky-otter",
    "gentle-acorn",
    "glassy-brook",
    "gleeful-sprout",
    "golden-drifter",
    "grassy-ember",
    "groovy-comet",
    "groovy-otter",
    "happy-harbor",
    "hardy-flint",
    "hazy-lantern",
    "hearty-fossil",
    "jolly-kettle",
    "jovial-pebble",
    "joyful-cedar",
    "kinetic-ripple",
    "lively-canoe",
    "loamy-sprout",
    "lucid-basin",
    "lush-cobalt",
    "lyric-fern",
    "mellow-comet",
    "merry-prism",
    "merry-sprout",
    "mellow-vale",
    "misty-anchor",
    "mossy-crater",
    "muted-flint",
    "nimble-delta",
    "peppy-gravel",
    "perky-nimbus",
    "pithy-fossil",
    "placid-atlas",
    "plucky-ember",
    "poky-quartz",
    "quick-harbor",
    "quirky-marble",
    "radiant-fern",
    "roomy-basin",
    "rosy-beacon",
    "sassy-comet",
    "silky-cedar",
    "snappy-anchor",
    "solar-prism",
    "spry-lantern",
    "sunny-brook",
    "wispy-cobalt",
    "zesty-delta",
    "zippy-sprout",
];

/// Pool for test-architect agents — 24 entries.
pub const TEST_ARCHITECT: &[&str] = &[
    "careful-crane",
    "careful-otter",
    "crisp-falcon",
    "diligent-badger",
    "exact-heron",
    "faithful-crane",
    "formal-egret",
    "honest-lapwing",
    "level-osprey",
    "linear-plover",
    "lucid-stork",
    "measured-swift",
    "meticulous-vole",
    "methodical-badger",
    "orderly-egret",
    "precise-otter",
    "precise-stork",
    "proper-crane",
    "rigorous-falcon",
    "steady-plover",
    "strict-ibis",
    "thorough-heron",
    "tidy-lapwing",
    "true-osprey",
];

/// Pool for reviewer agents — 24 entries.
pub const REVIEWER: &[&str] = &[
    "astute-magpie",
    "careful-heron",
    "candid-lynx",
    "clear-raven",
    "cool-magpie",
    "critical-kestrel",
    "earnest-puffin",
    "even-heron",
    "fair-raven",
    "fair-wren",
    "firm-kestrel",
    "frank-weasel",
    "honest-curlew",
    "impartial-finch",
    "keen-linnet",
    "level-magpie",
    "lucid-puffin",
    "perceptive-wren",
    "piercing-raven",
    "probing-linnet",
    "sharp-curlew",
    "sound-kestrel",
    "vigilant-lynx",
    "wary-merlin",
];

/// Pool for implementer agents — 24 entries.
pub const IMPLEMENTER: &[&str] = &[
    "agile-beaver",
    "bold-squirrel",
    "brisk-ferret",
    "busy-beaver",
    "capable-ferret",
    "crafty-fox",
    "deft-badger",
    "driven-marten",
    "eager-mink",
    "focused-otter",
    "handy-stoat",
    "keen-marten",
    "nimble-ferret",
    "productive-wombat",
    "prompt-vole",
    "quick-stoat",
    "rapid-mink",
    "ready-wombat",
    "sharp-squirrel",
    "skilled-beaver",
    "speedy-gecko",
    "steady-marten",
    "swift-beaver",
    "wired-gecko",
];

/// Pool for researcher agents — 24 entries.
pub const RESEARCHER: &[&str] = &[
    "broad-condor",
    "curious-finch",
    "curious-fulmar",
    "deep-condor",
    "earnest-ibis",
    "eager-fulmar",
    "explorative-wren",
    "inquisitive-mole",
    "learned-gannet",
    "musing-bittern",
    "open-gannet",
    "pensive-ibis",
    "prying-fulmar",
    "probing-mole",
    "pondering-bittern",
    "questioning-linnet",
    "reasoning-condor",
    "reflective-gannet",
    "seeking-bittern",
    "studied-egret",
    "studious-finch",
    "thoughtful-owl",
    "tracing-tern",
    "wide-tern",
];

/// Pool for debugger agents — 24 entries.
pub const DEBUGGER: &[&str] = &[
    "alert-lurcher",
    "astute-lurcher",
    "bright-hound",
    "clever-beagle",
    "clever-lurcher",
    "dogged-beagle",
    "ferreting-hound",
    "gritty-ferret",
    "hunting-beagle",
    "keen-bloodhound",
    "nosing-hound",
    "nosy-lurcher",
    "patient-beagle",
    "pointed-bloodhound",
    "prying-raven",
    "probing-beagle",
    "relentless-sleuth",
    "sharp-bloodhound",
    "sharp-sparrow",
    "sleuthing-hound",
    "sniffing-bloodhound",
    "sniffing-pointer",
    "tenacious-pointer",
    "tracing-sleuth",
];

// ── Generator ─────────────────────────────────────────────────────────────────

/// Process-local counter to guarantee uniqueness even on coarse clocks.
static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a stable funky name for an agent, session, or run.
///
/// `seed_ns` is a high-resolution UNIX timestamp in nanoseconds.  Two calls
/// in the same minute produce different `XX` suffixes with high probability.
///
/// Output format: `{adj}-{noun}-{HHMM}-{XX}` (e.g. `groovy-otter-1430-7f`).
pub fn generate_agent_name(category: AgentCategory, seed_ns: u64) -> String {
    let pool: &[&str] = match category {
        AgentCategory::Generic       => GENERIC,
        AgentCategory::TestArchitect => TEST_ARCHITECT,
        AgentCategory::Reviewer      => REVIEWER,
        AgentCategory::Implementer   => IMPLEMENTER,
        AgentCategory::Researcher    => RESEARCHER,
        AgentCategory::Debugger      => DEBUGGER,
    };

    // Index — mix seed_ns with the category discriminant.
    let mix = seed_ns
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (category as u64).wrapping_mul(0xc6a4_a793_5bd1_e995);
    let idx = (mix as usize) % pool.len();
    let adj_noun = pool[idx];

    // HHMM from seed (treat seed_ns as nanoseconds since UNIX epoch, UTC).
    let secs = seed_ns / 1_000_000_000;
    let secs_of_day = secs % 86_400;
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;

    // 2-char hex collision suffix using a different mixer (and the full seed
    // including subseconds, so two calls a single nanosecond apart differ).
    let h = seed_ns
        .rotate_left(13)
        .wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let suffix = (h >> 56) as u8;

    format!("{adj_noun}-{hh:02}{mm:02}-{suffix:02x}")
}

/// Convenience wrapper: parse `agent_type` and call `generate_agent_name`
/// with the current wall-clock nanosecond time as seed.
///
/// A process-local atomic counter is XORed into the seed to guarantee that
/// two consecutive calls return different names even on coarse-resolution
/// system clocks (e.g. CI environments where `SystemTime` has ms granularity).
pub fn name_for_agent_type(agent_type: Option<&str>) -> String {
    let category = match agent_type {
        Some(s) if !s.is_empty() => AgentCategory::from_agent_type(s),
        _ => AgentCategory::Generic,
    };
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // XOR with a monotonically-increasing counter so every call site gets a
    // unique seed regardless of clock resolution.
    let counter = CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed_ns = now_ns ^ counter.wrapping_mul(0x517c_c1b7_2722_0a95);
    generate_agent_name(category, seed_ns)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // All six pools combined for cross-pool uniqueness checks.
    fn all_pools() -> Vec<&'static str> {
        let mut v: Vec<&'static str> = Vec::new();
        v.extend_from_slice(GENERIC);
        v.extend_from_slice(TEST_ARCHITECT);
        v.extend_from_slice(REVIEWER);
        v.extend_from_slice(IMPLEMENTER);
        v.extend_from_slice(RESEARCHER);
        v.extend_from_slice(DEBUGGER);
        v
    }

    // ── Test 1 ────────────────────────────────────────────────────────────────
    // Fails TODAY: all pool consts are empty (&[]).
    // Passes AFTER green phase fills the pools.
    #[test]
    fn pool_constants_have_expected_sizes() {
        assert_eq!(GENERIC.len(), 80, "GENERIC pool must have 80 entries");
        assert_eq!(
            TEST_ARCHITECT.len(),
            24,
            "TEST_ARCHITECT pool must have 24 entries"
        );
        assert_eq!(REVIEWER.len(), 24, "REVIEWER pool must have 24 entries");
        assert_eq!(
            IMPLEMENTER.len(),
            24,
            "IMPLEMENTER pool must have 24 entries"
        );
        assert_eq!(RESEARCHER.len(), 24, "RESEARCHER pool must have 24 entries");
        assert_eq!(DEBUGGER.len(), 24, "DEBUGGER pool must have 24 entries");

        let total = GENERIC.len()
            + TEST_ARCHITECT.len()
            + REVIEWER.len()
            + IMPLEMENTER.len()
            + RESEARCHER.len()
            + DEBUGGER.len();
        assert_eq!(total, 200, "total pool size must be 200");
    }

    // ── Test 2 ────────────────────────────────────────────────────────────────
    // Fails TODAY: all pools are empty, so the loops never run and the
    // uniqueness assertion for 200 entries never holds — but more visibly,
    // if pools are ever partially filled the format assertions would catch bad
    // entries immediately.
    //
    // This test is written to ALSO fail when pools are empty, because the
    // final uniqueness assertion (all_entries.len() == 200) mirrors test 1.
    // Once the green phase fills pools, both tests pass together.
    #[test]
    fn pool_entries_are_well_formed() {
        let named_pools: &[(&str, &[&str])] = &[
            ("GENERIC", GENERIC),
            ("TEST_ARCHITECT", TEST_ARCHITECT),
            ("REVIEWER", REVIEWER),
            ("IMPLEMENTER", IMPLEMENTER),
            ("RESEARCHER", RESEARCHER),
            ("DEBUGGER", DEBUGGER),
        ];

        for (pool_name, pool) in named_pools {
            // Check format of every entry.
            for entry in *pool {
                // Must contain exactly one hyphen.
                let hyphen_count = entry.chars().filter(|&c| c == '-').count();
                assert_eq!(
                    hyphen_count, 1,
                    "pool {pool_name}: entry {entry:?} must contain exactly one hyphen"
                );

                // Every character must be lowercase ASCII letter or hyphen.
                assert!(
                    entry
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c == '-'),
                    "pool {pool_name}: entry {entry:?} contains non-lowercase-ASCII characters"
                );

                // Must not start or end with a hyphen.
                assert!(
                    !entry.starts_with('-') && !entry.ends_with('-'),
                    "pool {pool_name}: entry {entry:?} starts or ends with a hyphen"
                );
            }

            // No duplicates within this pool.
            let mut seen_within: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            for entry in *pool {
                assert!(
                    seen_within.insert(entry),
                    "pool {pool_name}: duplicate entry {entry:?}"
                );
            }
        }

        // No duplicates across all six pools combined.
        let all = all_pools();
        let mut seen_global: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for entry in &all {
            assert!(
                seen_global.insert(entry),
                "cross-pool duplicate entry {entry:?}"
            );
        }

        // Confirm total count — mirrors test 1 as a belt-and-suspenders check.
        assert_eq!(
            all.len(),
            200,
            "combined pool must have exactly 200 entries"
        );
    }

    // ── Test 3 ────────────────────────────────────────────────────────────────
    #[test]
    fn from_agent_type_known_routes() {
        let cases: &[(&str, AgentCategory)] = &[
            ("test-architect", AgentCategory::TestArchitect),
            ("code-reviewer", AgentCategory::Reviewer),
            ("code-reviewer-quick", AgentCategory::Reviewer),
            ("rust-expert", AgentCategory::Implementer),
            ("python-expert", AgentCategory::Implementer),
            ("flutter-expert", AgentCategory::Implementer),
            ("ml-recommendations-expert", AgentCategory::Implementer),
            ("ai-researcher", AgentCategory::Researcher),
            ("general-purpose", AgentCategory::Researcher),
            ("debugger", AgentCategory::Debugger),
            ("performance-optimizer", AgentCategory::Debugger),
            ("unknown-thing", AgentCategory::Generic),
            ("", AgentCategory::Generic),
        ];

        for (input, expected) in cases {
            let got = AgentCategory::from_agent_type(input);
            assert_eq!(
                got, *expected,
                "from_agent_type({input:?}) => {got:?}, want {expected:?}"
            );
        }
    }

    // ── Test 4 ────────────────────────────────────────────────────────────────
    #[test]
    fn generate_agent_name_format() {
        // seed_ns = 1_700_000_000_000_000_000
        // ≈ 2023-11-14T22:13:20 UTC  →  HHMM = "2213"
        let seed: u64 = 1_700_000_000_000_000_000;
        let name = generate_agent_name(AgentCategory::Generic, seed);

        let re = regex::Regex::new(r"^[a-z]+-[a-z]+-\d{4}-[0-9a-f]{2}$").unwrap();
        assert!(
            re.is_match(&name),
            "name {name:?} does not match expected format"
        );

        // The adj-noun part must appear in GENERIC pool.
        let parts: Vec<&str> = name.split('-').collect();
        // parts: [adj, noun, HHMM, XX]
        assert_eq!(parts.len(), 4, "expected 4 dash-separated segments in {name:?}");
        let adj_noun = format!("{}-{}", parts[0], parts[1]);
        assert!(
            GENERIC.contains(&adj_noun.as_str()),
            "adj-noun {adj_noun:?} not found in GENERIC pool"
        );
    }

    // ── Test 5 ────────────────────────────────────────────────────────────────
    #[test]
    fn generate_agent_name_deterministic() {
        let seed: u64 = 1_700_000_000_000_000_000;
        let first = generate_agent_name(AgentCategory::Generic, seed);
        let second = generate_agent_name(AgentCategory::Generic, seed);
        assert_eq!(first, second, "same seed must produce identical names");
    }

    // ── Test 6 ────────────────────────────────────────────────────────────────
    #[test]
    fn generate_agent_name_collision_suffix_differs() {
        let seed_a: u64 = 1_700_000_000_000_000_000;
        let seed_b: u64 = seed_a + 1; // one nanosecond later

        let name_a = generate_agent_name(AgentCategory::Generic, seed_a);
        let name_b = generate_agent_name(AgentCategory::Generic, seed_b);

        // Extract the last two-char hex suffix.
        let suffix_a = name_a.split('-').last().unwrap();
        let suffix_b = name_b.split('-').last().unwrap();

        assert_ne!(
            suffix_a, suffix_b,
            "suffixes for seeds one nanosecond apart must differ; got {name_a:?} vs {name_b:?}"
        );
    }

    // ── Test 7 ────────────────────────────────────────────────────────────────
    #[test]
    fn generate_agent_name_uses_correct_pool() {
        let seed: u64 = 1_700_000_000_000_000_000;
        let name = generate_agent_name(AgentCategory::TestArchitect, seed);

        // Extract adj-noun (first two hyphen-delimited segments).
        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 4, "expected 4 segments in {name:?}");
        let adj_noun = format!("{}-{}", parts[0], parts[1]);

        assert!(
            TEST_ARCHITECT.contains(&adj_noun.as_str()),
            "adj-noun {adj_noun:?} must come from TEST_ARCHITECT, not GENERIC"
        );
        assert!(
            !GENERIC.contains(&adj_noun.as_str()),
            "adj-noun {adj_noun:?} must NOT be in GENERIC for TestArchitect category"
        );
    }

    // ── Test 8 ────────────────────────────────────────────────────────────────
    #[test]
    fn name_for_agent_type_falls_back_to_generic() {
        // None → Generic pool.
        let name_none = name_for_agent_type(None);
        let parts_none: Vec<&str> = name_none.split('-').collect();
        assert_eq!(parts_none.len(), 4, "expected 4 segments in {name_none:?}");
        let adj_noun_none = format!("{}-{}", parts_none[0], parts_none[1]);
        assert!(
            GENERIC.contains(&adj_noun_none.as_str()),
            "None fallback must use GENERIC pool; got {name_none:?}"
        );

        // Some("???") → Generic pool (unknown type).
        let name_unknown = name_for_agent_type(Some("???"));
        let parts_unknown: Vec<&str> = name_unknown.split('-').collect();
        assert_eq!(
            parts_unknown.len(),
            4,
            "expected 4 segments in {name_unknown:?}"
        );
        let adj_noun_unknown = format!("{}-{}", parts_unknown[0], parts_unknown[1]);
        assert!(
            GENERIC.contains(&adj_noun_unknown.as_str()),
            "unknown type '???' must use GENERIC pool; got {name_unknown:?}"
        );
    }

    // ── Test 9 ────────────────────────────────────────────────────────────────
    #[test]
    fn generate_agent_name_hhmm_matches_seed() {
        // seed_ns = 1_700_000_000_000_000_000
        // Unix seconds = 1_700_000_000
        // 1_700_000_000 / 3600 % 24 = 22  (hours UTC)
        // 1_700_000_000 % 3600 / 60 = 13  (minutes UTC)
        // → HHMM = "2213"
        let seed: u64 = 1_700_000_000_000_000_000;
        let name = generate_agent_name(AgentCategory::Generic, seed);

        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 4, "expected 4 segments in {name:?}");
        let hhmm = parts[2];
        assert_eq!(hhmm, "2213", "HHMM segment must be '2213' for the pinned seed; got {hhmm:?} in {name:?}");
    }
}
