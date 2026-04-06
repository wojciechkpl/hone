use std::collections::HashMap;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Parsed representation of a `HONE.md` conventions file.
///
/// Only level-2 (`##`) headings are treated as section keys.  Everything
/// between two consecutive h2 headings (or between the last h2 and EOF) is
/// collected as the section's raw text content.
#[derive(Debug, Clone, PartialEq)]
pub struct HoneMd {
    /// Sections keyed by their heading text (e.g. `"TOOLS"`, `"STYLE"`).
    pub sections: HashMap<String, String>,
}

impl HoneMd {
    /// Parse a `HONE.md` markdown string into sections.
    pub fn parse(markdown: &str) -> Self {
        let mut sections: HashMap<String, String> = HashMap::new();

        if markdown.is_empty() {
            return Self { sections };
        }

        let parser = Parser::new_ext(markdown, Options::empty());

        // State machine: we track whether we are currently inside an h2 heading
        // and, once we've left the heading, we accumulate text events.
        let mut current_section: Option<String> = None;
        let mut current_heading_text = String::new();
        let mut in_h2_heading = false;
        let mut current_content = String::new();

        for event in parser {
            match event {
                Event::Start(Tag::Heading { level: HeadingLevel::H2, .. }) => {
                    // Flush the previous section if any.
                    if let Some(key) = current_section.take() {
                        sections.insert(key, current_content.trim().to_owned());
                        current_content = String::new();
                    }
                    in_h2_heading = true;
                    current_heading_text.clear();
                }

                Event::End(TagEnd::Heading(HeadingLevel::H2)) => {
                    in_h2_heading = false;
                    let key = current_heading_text.trim().to_owned();
                    if !key.is_empty() {
                        current_section = Some(key);
                    }
                    current_heading_text.clear();
                }

                Event::Text(text) if in_h2_heading => {
                    current_heading_text.push_str(&text);
                }

                Event::Text(text) => {
                    if current_section.is_some() {
                        current_content.push_str(&text);
                        current_content.push('\n');
                    }
                }

                Event::SoftBreak | Event::HardBreak => {
                    if current_section.is_some() && !in_h2_heading {
                        current_content.push('\n');
                    }
                }

                // Ignore all other events (Start/End tags for paragraphs,
                // lists, code blocks, etc.) but still collect their text via
                // the Text variant above.
                _ => {}
            }
        }

        // Flush the final section.
        if let Some(key) = current_section {
            sections.insert(key, current_content.trim().to_owned());
        }

        Self { sections }
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn basic_two_sections() {
        let md = "## ALPHA\n\nhello\n\n## BETA\n\nworld\n";
        let hone = HoneMd::parse(md);
        assert_eq!(hone.sections["ALPHA"], "hello");
        assert_eq!(hone.sections["BETA"], "world");
    }

    #[test]
    fn h1_is_ignored() {
        let md = "# Top\n\n## REAL\n\ncontent\n";
        let hone = HoneMd::parse(md);
        assert_eq!(hone.sections.len(), 1);
        assert!(hone.sections.contains_key("REAL"));
    }

    #[test]
    fn test_hone_md_parse_and_inject() {
        let md = "## TOOLS\n\nUse ripgrep for search.\n\n## STYLE\n\nPrefer short functions.\n";
        let hone = HoneMd::parse(md);

        // Build a system prompt by injecting every section.
        let mut system_prompt = "You are Hone, an AI coding assistant.".to_string();
        let mut section_keys: Vec<&String> = hone.sections.keys().collect();
        section_keys.sort(); // deterministic order for assertions
        for key in &section_keys {
            let content = &hone.sections[*key];
            system_prompt.push_str(&format!("\n\n## {key}\n{content}"));
        }

        assert!(
            system_prompt.contains("You are Hone"),
            "system prompt must contain base text"
        );
        assert!(
            system_prompt.contains("## STYLE"),
            "system prompt must contain STYLE section heading"
        );
        assert!(
            system_prompt.contains("Prefer short functions"),
            "system prompt must contain STYLE section content"
        );
        assert!(
            system_prompt.contains("## TOOLS"),
            "system prompt must contain TOOLS section heading"
        );
        assert!(
            system_prompt.contains("Use ripgrep for search"),
            "system prompt must contain TOOLS section content"
        );
    }
}
