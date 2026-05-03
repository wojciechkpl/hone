use crate::model::{AcceptanceCriterion, Task, TaskStatus};

/// Parse a tasks.md document into a Vec<Task>. Tolerant of missing optional
/// metadata: depends_on defaults to [], status defaults to Pending.
/// Returns an empty vec if no `## T...` headings are found (NOT an error).
pub fn parse_tasks_md(md: &str) -> Vec<Task> {
    let mut tasks: Vec<Task> = Vec::new();

    // We'll accumulate lines per task using an Option<TaskBuilder>
    struct TaskBuilder {
        id: String,
        title: String,
        depends_on: Vec<String>,
        status: TaskStatus,
        body_lines: Vec<String>,
    }

    let mut current: Option<TaskBuilder> = None;

    let flush = |builder: TaskBuilder| -> Task {
        // Split body into description (before acceptance criteria) and criteria
        let mut description_lines: Vec<String> = Vec::new();
        let mut criteria: Vec<AcceptanceCriterion> = Vec::new();
        let mut in_criteria = false;

        for line in &builder.body_lines {
            if !in_criteria && line.trim() == "**Acceptance criteria:**" {
                in_criteria = true;
                continue;
            }
            if in_criteria {
                if let Some(rest) = line.strip_prefix("- [ ] ") {
                    criteria.push(AcceptanceCriterion {
                        text: rest.trim().to_owned(),
                        done: false,
                    });
                } else if let Some(rest) = line.strip_prefix("- [x] ") {
                    criteria.push(AcceptanceCriterion {
                        text: rest.trim().to_owned(),
                        done: true,
                    });
                }
                // Other lines in criteria section are ignored
            } else {
                description_lines.push(line.clone());
            }
        }

        // Trim leading/trailing blank lines from description
        let desc = description_lines
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let desc = desc.trim().to_owned();

        Task {
            id: builder.id,
            title: builder.title,
            description: desc,
            acceptance_criteria: criteria,
            depends_on: builder.depends_on,
            status: builder.status,
        }
    };

    let mut lines = md.lines().peekable();

    while let Some(line) = lines.next() {
        // Check for task heading: ## T<digits> [—|-] <title>
        if let Some(rest) = line.strip_prefix("## ") {
            // Try to parse as T<NNN> <sep> <title>
            if let Some((id, title)) = parse_heading(rest) {
                // Flush previous task
                if let Some(builder) = current.take() {
                    tasks.push(flush(builder));
                }

                // Read optional metadata comments immediately following the heading
                let mut depends_on: Vec<String> = Vec::new();
                let mut status = TaskStatus::Pending;
                let mut body_lines: Vec<String> = Vec::new();

                // Peek ahead for HTML comment metadata lines
                // (they may or may not be present, order: depends_on then status)
                'meta: loop {
                    match lines.peek() {
                        Some(&next_line) if next_line.trim_start().starts_with("<!-- ") => {
                            let trimmed = next_line.trim();
                            if let Some(inner) = trimmed
                                .strip_prefix("<!-- ")
                                .and_then(|s| s.strip_suffix(" -->"))
                            {
                                if let Some(val) = inner.strip_prefix("depends_on:") {
                                    let json = val.trim();
                                    if let Ok(v) =
                                        serde_json::from_str::<Vec<String>>(json)
                                    {
                                        depends_on = v;
                                    }
                                    lines.next();
                                    continue 'meta;
                                } else if let Some(val) = inner.strip_prefix("status:") {
                                    status = TaskStatus::from_str(val.trim())
                                        .unwrap_or(TaskStatus::Pending);
                                    lines.next();
                                    continue 'meta;
                                }
                            }
                            break 'meta;
                        }
                        _ => break 'meta,
                    }
                }

                current = Some(TaskBuilder {
                    id,
                    title,
                    depends_on,
                    status,
                    body_lines,
                });
                continue;
            }
        }

        // Accumulate body lines for the current task
        if let Some(ref mut builder) = current {
            builder.body_lines.push(line.to_owned());
        }
        // Lines before any task heading are silently ignored
    }

    // Flush the last task
    if let Some(builder) = current.take() {
        tasks.push(flush(builder));
    }

    tasks
}

/// Parse `"T042 — Some title"` or `"T042 - Some title"` into `("T042", "Some title")`.
fn parse_heading(rest: &str) -> Option<(String, String)> {
    // Match T followed by digits, then space + separator + space + title
    // Separators: em-dash (U+2014) or regular hyphen
    let separators = [" \u{2014} ", " - "];
    for sep in &separators {
        if let Some(pos) = rest.find(sep) {
            let id_part = &rest[..pos];
            // Validate: must start with T followed by digits
            if id_part.starts_with('T') && id_part[1..].chars().all(|c| c.is_ascii_digit()) {
                let title = rest[pos + sep.len()..].trim().to_owned();
                return Some((id_part.to_owned(), title));
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    // --------------------------------------------------------------------------
    // Empty / no-heading cases
    // --------------------------------------------------------------------------

    #[test]
    fn parse_empty_returns_empty_vec() {
        let tasks = parse_tasks_md("");
        assert!(tasks.is_empty());
    }

    #[test]
    fn parse_no_task_headings_returns_empty_vec() {
        let md = "# Tasks\n\nSome introductory text.\n\nAnother paragraph.\n";
        let tasks = parse_tasks_md(md);
        assert!(tasks.is_empty());
    }

    // --------------------------------------------------------------------------
    // Single task: metadata presence/absence
    // --------------------------------------------------------------------------

    #[test]
    fn parse_single_task_minimal_metadata() {
        // Only the heading — no HTML comments for depends_on or status.
        let md = "# Tasks\n\n## T001 — Set up auth schema\n\nSome body text.\n";
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!(t.id, "T001");
        assert_eq!(t.title, "Set up auth schema");
        assert_eq!(t.status, TaskStatus::Pending);
        assert!(t.depends_on.is_empty());
    }

    #[test]
    fn parse_single_task_full_metadata() {
        let md = concat!(
            "# Tasks\n\n",
            "## T001 — Set up auth schema\n",
            "<!-- depends_on: [] -->\n",
            "<!-- status: pending -->\n",
            "\n",
            "Create the `users` table.\n"
        );
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!(t.id, "T001");
        assert_eq!(t.title, "Set up auth schema");
        assert_eq!(t.status, TaskStatus::Pending);
        assert!(t.depends_on.is_empty());
        assert!(t.description.contains("Create the `users` table."));
    }

    // --------------------------------------------------------------------------
    // Multiple tasks
    // --------------------------------------------------------------------------

    #[test]
    fn parse_multiple_tasks_in_order() {
        let md = concat!(
            "# Tasks\n\n",
            "## T001 — First task\n",
            "<!-- depends_on: [] -->\n",
            "<!-- status: pending -->\n",
            "\n",
            "Body one.\n\n",
            "## T002 — Second task\n",
            "<!-- depends_on: [\"T001\"] -->\n",
            "<!-- status: in_progress -->\n",
            "\n",
            "Body two.\n"
        );
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "T001");
        assert_eq!(tasks[1].id, "T002");
        // Second task should not bleed into first task's body
        assert!(!tasks[0].description.contains("Body two"));
    }

    // --------------------------------------------------------------------------
    // Acceptance criteria
    // --------------------------------------------------------------------------

    #[test]
    fn parse_acceptance_criteria_with_mixed_done_states() {
        let md = concat!(
            "## T001 — Example\n",
            "<!-- depends_on: [] -->\n",
            "<!-- status: pending -->\n",
            "\n",
            "**Acceptance criteria:**\n",
            "- [ ] Not done yet\n",
            "- [x] Already done\n",
            "- [ ] Also pending\n"
        );
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks.len(), 1);
        let criteria = &tasks[0].acceptance_criteria;
        assert_eq!(criteria.len(), 3);
        assert!(!criteria[0].done);
        assert_eq!(criteria[0].text, "Not done yet");
        assert!(criteria[1].done);
        assert_eq!(criteria[1].text, "Already done");
        assert!(!criteria[2].done);
    }

    // --------------------------------------------------------------------------
    // Dependency parsing
    // --------------------------------------------------------------------------

    #[test]
    fn parse_dependencies_json_array() {
        let md = concat!(
            "## T002 — Second\n",
            "<!-- depends_on: [\"T001\"] -->\n",
            "<!-- status: pending -->\n",
            "\n",
            "Body.\n"
        );
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].depends_on, vec!["T001".to_owned()]);
    }

    // --------------------------------------------------------------------------
    // Status variants
    // --------------------------------------------------------------------------

    #[test]
    fn parse_status_in_progress() {
        let md = "## T001 — A task\n<!-- depends_on: [] -->\n<!-- status: in_progress -->\n\nBody.\n";
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks[0].status, TaskStatus::InProgress);
    }

    #[test]
    fn parse_status_done() {
        let md = "## T001 — A task\n<!-- depends_on: [] -->\n<!-- status: done -->\n\nBody.\n";
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks[0].status, TaskStatus::Done);
    }

    #[test]
    fn parse_status_blocked() {
        let md = "## T001 — A task\n<!-- depends_on: [] -->\n<!-- status: blocked -->\n\nBody.\n";
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks[0].status, TaskStatus::Blocked);
    }

    #[test]
    fn parse_invalid_status_falls_back_to_pending() {
        let md = "## T001 — A task\n<!-- depends_on: [] -->\n<!-- status: totally_invalid -->\n\nBody.\n";
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks[0].status, TaskStatus::Pending);
    }

    // --------------------------------------------------------------------------
    // Heading separator variants
    // --------------------------------------------------------------------------

    #[test]
    fn parse_em_dash_separator_works() {
        // em-dash: U+2014 surrounded by spaces: " — "
        let md = "## T001 \u{2014} Em dash title\n<!-- depends_on: [] -->\n<!-- status: pending -->\n\nBody.\n";
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "T001");
        assert_eq!(tasks[0].title, "Em dash title");
    }

    #[test]
    fn parse_regular_hyphen_separator_also_works() {
        // plain hyphen: " - "
        let md = "## T001 - Hyphen title\n<!-- depends_on: [] -->\n<!-- status: pending -->\n\nBody.\n";
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "T001");
        assert_eq!(tasks[0].title, "Hyphen title");
    }

    // --------------------------------------------------------------------------
    // ID extraction
    // --------------------------------------------------------------------------

    #[test]
    fn parse_id_extracted_from_heading() {
        let md = "## T042 — Some title\n<!-- depends_on: [] -->\n<!-- status: pending -->\n\nBody.\n";
        let tasks = parse_tasks_md(md);
        assert_eq!(tasks[0].id, "T042");
    }
}
