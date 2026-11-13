# The injection detector sees through hidden characters: spec

Branch `fix/injection-hidden-chars`, off `main`. Found while reviewing workspace document retrieval (#115). There, a trigger phrase split by a control or zero-width character, or written in Unicode tag characters, got past the detector. `InjectionDetector::scan` matches regexes against the raw text, so tool results, tool errors and MCP output have the same blind spot.

## Rows

| Row | Behaviour |
|---|---|
| TR1 | **Matching ignores hidden characters.** `InjectionDetector::scan` matches its patterns against a normalized copy of the text. In that copy: control characters are removed, with newlines and tabs becoming spaces; Unicode format (Cf) characters are removed; NFKD is applied; and nonspacing marks (Mn) are removed. A trigger phrase split by any of these is caught: BEL, a C1 control, U+200B, U+200C, U+200D, U+2060, U+FEFF, U+00AD, a combining mark, U+034F, a variation selector, fullwidth or math-bold letters. This applies wherever `scan` is used (tool results, tool errors, observed results). |
| TR2 | **Hidden tag text is scanned.** Unicode tag characters U+E0020–U+E007E are also decoded to their ASCII counterparts (U+0020–U+007E), and that decoded text is scanned. An instruction written invisibly in tag characters is matched like a visible one. |
| TR3 | **Tag characters alone are a warning.** A tool result that contains any tag character (U+E0000–E007F) is at least a medium risk, labelled as hidden Unicode tag characters, even when the decoded text matches no pattern. So it gets the existing SECURITY prefix. The result's content isn't changed, because the model needs exact bytes to edit files. |
| TR4 | **ASCII stays fast.** Text that is entirely ASCII is scanned as before, with no normalized copy allocated. Scanning stays linear in the text's length. |

## Testing notes

- Unit tests in `injection_detector.rs` or `crates/hone-core/tests/`: the variants in TR1, a tag-encoded phrase for TR2, tag characters with a benign payload for TR3, and benign non-Latin text (Persian with ZWNJ, Hindi with ZWJ, Vietnamese) that isn't flagged.
- One agent-level test: a tool result with a split phrase gets the SECURITY prefix in history.
- TR4: a structural test that ASCII input takes the fast path, if it's observable; otherwise a timing guard on a 5 MB ASCII string.

## Out of scope

- Homoglyphs from other scripts, such as a Cyrillic `І`.
- Changing tool-result content.
