//! RED-PHASE regression tests for **HIGH-2**: `tee` (and other write-capable
//! tools) on the safe-pipe allowlist let writes bypass the
//! `Mutation`/`Destructive` classifier and therefore the user-approval prompt.
//!
//! Audit finding (b):
//!     `crates/hone-tools/src/shell/validation.rs:230-237`
//!     The allowlist `["head","tail","grep","wc","sort","uniq","less","more",
//!     "cat","cut","awk","sed","tr","tee","column","fmt","nl","rev","tac",
//!     "fold","expand","unexpand"]` includes `tee` (a writer) and `awk`/`sed`
//!     which both accept output-redirect flags (`-i`, `{print > "file"}`).
//!     A pipe like `ls /etc | tee /home/user/.bashrc` is therefore classified
//!     as ReadOnly + safe-pipe, skipping the approval prompt entirely.
//!
//! Contract these tests pin:
//!     A pipe whose target writes to the filesystem MUST escalate the
//!     classification to at least `Mutation` (and ideally `Destructive`),
//!     and `validate_command` must NOT return `Allow` for such pipes.
//!
//! Implementer's freedom (green phase):
//!     - Drop `tee` from `safe_pipe_targets`.
//!     - For `awk`/`sed`, add a flag-aware sub-check so `-i`, `--in-place`,
//!       and shell-redirection-in-script are not silently allowed.

#![allow(clippy::expect_used, clippy::unwrap_used)]

// `validate_command` is `pub fn` but lives behind a private `mod shell` in
// `lib.rs`, so it cannot be reached from integration tests.  We exercise the
// contract through `CommandIntent::classify`, which IS re-exported and which
// the validator pipeline drives directly: an intent that escalates above
// `ReadOnly` causes `validate_command` to return `RequireApproval` or `Block`
// rather than `Allow`.  Pinning the classifier therefore pins the validator.
use hone_tools::CommandIntent;

// ---------------------------------------------------------------------------
// Finding (b.1): `tee` is a writer and must NOT be safe-piped.
// ---------------------------------------------------------------------------

#[test]
fn pipe_to_tee_is_not_readonly() {
    // The classic exfiltration pattern.  Today this is classified ReadOnly
    // because `cat` is read-only and the pipe target `tee` is on the
    // safe-pipe allowlist.
    let cmd = "cat /etc/hosts | tee /tmp/exfil";
    let intent = CommandIntent::classify(cmd);

    assert_ne!(
        intent,
        CommandIntent::ReadOnly,
        "AUDIT-HIGH-2: '{cmd}' must NOT be classified ReadOnly — \
         tee writes its stdin to a file and bypasses the approval gate"
    );
}

#[test]
fn pipe_to_tee_targets_at_least_mutation() {
    // The escalation must be at least Mutation (which causes the validator
    // to return Allow-with-sandbox-scoped) and ideally Destructive
    // (RequireApproval).  Either is acceptable; ReadOnly is not.
    let cmd = "ls /etc | tee /home/user/.bashrc";
    let intent = CommandIntent::classify(cmd);

    assert!(
        matches!(
            intent,
            CommandIntent::Mutation | CommandIntent::Destructive | CommandIntent::Unknown
        ),
        "AUDIT-HIGH-2: pipe to tee must escalate at least to Mutation, got {intent:?}"
    );
}

#[test]
fn pipe_to_tee_with_append_flag_is_not_readonly() {
    // `tee -a` appends instead of truncating, but it is still a write.
    let cmd = "echo evil | tee -a /etc/profile";
    let intent = CommandIntent::classify(cmd);

    assert_ne!(
        intent,
        CommandIntent::ReadOnly,
        "AUDIT-HIGH-2: 'tee -a' is still a write; must not be ReadOnly"
    );
}

// ---------------------------------------------------------------------------
// Finding (b.2): `awk` with in-place output redirect must be caught.
// ---------------------------------------------------------------------------

#[test]
fn pipe_to_awk_with_output_redirect_is_not_readonly() {
    // GNU awk accepts `{print > "file"}` inside the program; this writes
    // arbitrary content to the named file.  The first-token classifier
    // cannot see this because awk's program is a quoted argument.
    let cmd = r#"cat input | awk '{print > "/tmp/owned.txt"}'"#;
    let intent = CommandIntent::classify(cmd);

    assert_ne!(
        intent,
        CommandIntent::ReadOnly,
        "AUDIT-HIGH-2: awk programs containing output redirection (> \"file\") \
         write to the filesystem; must not be ReadOnly"
    );
}

// ---------------------------------------------------------------------------
// Finding (b.3): `sed -i` mutates files in place.
// ---------------------------------------------------------------------------

#[test]
fn pipe_to_sed_with_in_place_flag_is_not_readonly() {
    let cmd = "echo hi | sed -i 's/old/new/g' /etc/motd";
    let intent = CommandIntent::classify(cmd);

    assert_ne!(
        intent,
        CommandIntent::ReadOnly,
        "AUDIT-HIGH-2: 'sed -i' edits files in place; must not be ReadOnly \
         when reachable via a safe-pipe escape"
    );
}

#[test]
fn pipe_to_sed_with_long_in_place_flag_is_not_readonly() {
    let cmd = "echo hi | sed --in-place 's/old/new/g' /etc/motd";
    let intent = CommandIntent::classify(cmd);

    assert_ne!(
        intent,
        CommandIntent::ReadOnly,
        "AUDIT-HIGH-2: 'sed --in-place' is the long form of '-i'; same hazard"
    );
}

// ---------------------------------------------------------------------------
// Finding (b.4): negative-control / regression guards.
// ---------------------------------------------------------------------------

#[test]
fn pipe_to_head_remains_readonly() {
    // The whole point of the safe-pipe allowlist is that `cmd | head` still
    // works without a prompt.  After the green-phase fix this MUST still
    // hold — head is a true read-only consumer.
    let cmd = "ls /tmp | head -20";
    let intent = CommandIntent::classify(cmd);

    assert_eq!(
        intent,
        CommandIntent::ReadOnly,
        "regression: pipe to 'head' must remain ReadOnly after the fix"
    );
}

#[test]
fn pipe_to_grep_remains_readonly() {
    let cmd = "cat foo.log | grep ERROR";
    let intent = CommandIntent::classify(cmd);

    assert_eq!(
        intent,
        CommandIntent::ReadOnly,
        "regression: pipe to 'grep' must remain ReadOnly after the fix"
    );
}

#[test]
fn pipe_to_wc_remains_readonly() {
    let cmd = "find . -name '*.rs' | wc -l";
    let intent = CommandIntent::classify(cmd);

    assert_eq!(
        intent,
        CommandIntent::ReadOnly,
        "regression: pipe to 'wc' must remain ReadOnly after the fix"
    );
}
