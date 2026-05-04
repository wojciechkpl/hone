use crate::model::Task;

/// Render a Vec<Task> as tasks.md. The round-trip property is:
///   parse(render(tasks)) == tasks
/// (modulo whitespace normalisation in the body)
pub fn render_tasks_md(tasks: &[Task]) -> String {
    let mut out = String::from("# Tasks\n");

    for task in tasks {
        out.push('\n');

        // Heading with em-dash separator
        out.push_str(&format!("## {} \u{2014} {}\n", task.id, task.title));

        // Metadata comments
        let deps_json = serde_json::to_string(&task.depends_on)
            .unwrap_or_else(|_| "[]".to_owned());
        out.push_str(&format!("<!-- depends_on: {} -->\n", deps_json));
        out.push_str(&format!("<!-- status: {} -->\n", task.status.as_str()));

        out.push('\n');

        // Description body
        if !task.description.is_empty() {
            out.push_str(&task.description);
            out.push('\n');
        }

        // Acceptance criteria section
        if !task.acceptance_criteria.is_empty() {
            out.push('\n');
            out.push_str("**Acceptance criteria:**\n");
            for criterion in &task.acceptance_criteria {
                let checkbox = if criterion.done { "[x]" } else { "[ ]" };
                out.push_str(&format!("- {} {}\n", checkbox, criterion.text));
            }
        }
    }

    out
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::model::{AcceptanceCriterion, TaskStatus};
    use crate::parse::parse_tasks_md;

    fn make_task(id: &str, title: &str, status: TaskStatus) -> Task {
        Task {
            id: id.to_owned(),
            title: title.to_owned(),
            description: "Some description text.".to_owned(),
            acceptance_criteria: vec![],
            depends_on: vec![],
            status,
        }
    }

    // --------------------------------------------------------------------------
    // Basic render output
    // --------------------------------------------------------------------------

    #[test]
    fn render_empty_returns_header_only() {
        let output = render_tasks_md(&[]);
        // Must at least have the top-level heading
        assert!(output.contains("# Tasks"));
        // Must not contain any task heading (## T...)
        assert!(!output.contains("## T"));
    }

    #[test]
    fn render_single_task_includes_title_and_body() {
        let task = Task {
            id: "T001".to_owned(),
            title: "Set up auth schema".to_owned(),
            description: "Create the users table.".to_owned(),
            acceptance_criteria: vec![],
            depends_on: vec![],
            status: TaskStatus::Pending,
        };
        let output = render_tasks_md(&[task]);
        assert!(output.contains("T001"));
        assert!(output.contains("Set up auth schema"));
        assert!(output.contains("Create the users table."));
    }

    #[test]
    fn render_includes_status_metadata() {
        let task = make_task("T001", "A task", TaskStatus::InProgress);
        let output = render_tasks_md(&[task]);
        assert!(output.contains("in_progress") || output.contains("in-progress"));
    }

    #[test]
    fn render_includes_depends_on_metadata() {
        let task = Task {
            id: "T002".to_owned(),
            title: "Second task".to_owned(),
            description: "Does something.".to_owned(),
            acceptance_criteria: vec![],
            depends_on: vec!["T001".to_owned()],
            status: TaskStatus::Pending,
        };
        let output = render_tasks_md(&[task]);
        assert!(output.contains("T001"));
        assert!(output.contains("depends_on"));
    }

    #[test]
    fn render_includes_acceptance_criteria_with_correct_checkboxes() {
        let task = Task {
            id: "T001".to_owned(),
            title: "With criteria".to_owned(),
            description: "Body.".to_owned(),
            acceptance_criteria: vec![
                AcceptanceCriterion {
                    text: "Not done".to_owned(),
                    done: false,
                },
                AcceptanceCriterion {
                    text: "Already done".to_owned(),
                    done: true,
                },
            ],
            depends_on: vec![],
            status: TaskStatus::Pending,
        };
        let output = render_tasks_md(&[task]);
        assert!(output.contains("- [ ] Not done"));
        assert!(output.contains("- [x] Already done"));
    }

    // --------------------------------------------------------------------------
    // Round-trip: render then parse preserves all fields
    // --------------------------------------------------------------------------

    fn sample_tasks() -> Vec<Task> {
        vec![
            Task {
                id: "T001".to_owned(),
                title: "First task".to_owned(),
                description: "Do the first thing.".to_owned(),
                acceptance_criteria: vec![
                    AcceptanceCriterion {
                        text: "It works".to_owned(),
                        done: false,
                    },
                    AcceptanceCriterion {
                        text: "Tests pass".to_owned(),
                        done: true,
                    },
                ],
                depends_on: vec![],
                status: TaskStatus::Pending,
            },
            Task {
                id: "T002".to_owned(),
                title: "Second task".to_owned(),
                description: "Do the second thing.".to_owned(),
                acceptance_criteria: vec![AcceptanceCriterion {
                    text: "Compiles".to_owned(),
                    done: false,
                }],
                depends_on: vec!["T001".to_owned()],
                status: TaskStatus::InProgress,
            },
        ]
    }

    #[test]
    fn roundtrip_render_then_parse_preserves_ids() {
        let original = sample_tasks();
        let rendered = render_tasks_md(&original);
        let parsed = parse_tasks_md(&rendered);
        let ids: Vec<&str> = parsed.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["T001", "T002"]);
    }

    #[test]
    fn roundtrip_render_then_parse_preserves_titles() {
        let original = sample_tasks();
        let rendered = render_tasks_md(&original);
        let parsed = parse_tasks_md(&rendered);
        assert_eq!(parsed[0].title, "First task");
        assert_eq!(parsed[1].title, "Second task");
    }

    #[test]
    fn roundtrip_render_then_parse_preserves_status() {
        let original = sample_tasks();
        let rendered = render_tasks_md(&original);
        let parsed = parse_tasks_md(&rendered);
        assert_eq!(parsed[0].status, TaskStatus::Pending);
        assert_eq!(parsed[1].status, TaskStatus::InProgress);
    }

    #[test]
    fn roundtrip_render_then_parse_preserves_depends_on() {
        let original = sample_tasks();
        let rendered = render_tasks_md(&original);
        let parsed = parse_tasks_md(&rendered);
        assert!(parsed[0].depends_on.is_empty());
        assert_eq!(parsed[1].depends_on, vec!["T001".to_owned()]);
    }

    #[test]
    fn roundtrip_render_then_parse_preserves_acceptance_criteria() {
        let original = sample_tasks();
        let rendered = render_tasks_md(&original);
        let parsed = parse_tasks_md(&rendered);
        let criteria = &parsed[0].acceptance_criteria;
        assert_eq!(criteria.len(), 2);
        assert_eq!(criteria[0].text, "It works");
        assert!(!criteria[0].done);
        assert_eq!(criteria[1].text, "Tests pass");
        assert!(criteria[1].done);
    }
}
