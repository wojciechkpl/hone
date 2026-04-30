/// A structured body for a typed memory entry.
///
/// `Feedback` and `Project` memories are rendered with all three sections
/// (omitting any that are `None`).  All other kinds return `rule` unchanged.
#[derive(Debug)]
pub struct StructuredBody {
    pub rule: String,
    pub why: Option<String>,
    pub how: Option<String>,
}

/// Render a `StructuredBody` into a string.
///
/// For [`MemoryKind::Feedback`] and [`MemoryKind::Project`]:
/// ```text
/// {rule}
///
/// **Why:** {why}
/// **How to apply:** {how}
/// ```
/// Any section whose field is `None` is omitted entirely.
///
/// For all other kinds, returns `body.rule` unchanged.
pub fn format_memory_body(kind: hone_memory::MemoryKind, body: StructuredBody) -> String {
    use hone_memory::MemoryKind;

    match kind {
        MemoryKind::Feedback | MemoryKind::Project => {
            let mut out = body.rule;
            if let Some(why) = body.why {
                out.push_str("\n\n**Why:** ");
                out.push_str(&why);
            }
            if let Some(how) = body.how {
                out.push('\n');
                out.push_str("**How to apply:** ");
                out.push_str(&how);
            }
            out
        }
        _ => body.rule,
    }
}
