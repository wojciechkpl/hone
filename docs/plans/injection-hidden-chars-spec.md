# The injection detector sees through hidden characters: spec

Branch `fix/injection-hidden-chars`, off `main`. Found while reviewing workspace document retrieval (#115). There, a trigger phrase split by a control or zero-width character, or written in Unicode tag characters, got past the detector. `InjectionDetector::scan` matches regexes against the raw text, so tool results, tool errors and MCP output have the same blind spot.

## Rows

| Row | Behaviour |
|---|---|
| TR1 | **Matching ignores hidden characters.** `InjectionDetector::scan` matches its patterns against a normalized copy of the text. In that copy: control characters are removed, with newlines and tabs becoming spaces; Unicode format (Cf) characters are removed; NFKD is applied; and nonspacing marks (Mn) are removed. A trigger phrase split by any of these is caught: BEL, a C1 control, U+200B, U+200C, U+200D, U+2060, U+FEFF, U+00AD, a combining mark, U+034F, a variation selector, fullwidth or math-bold letters. This applies wherever `scan` is used (tool results, tool errors, observed results). |
| TR2 | **Hidden tag text is scanned.** Unicode tag characters U+E0020–U+E007E are also decoded to their ASCII counterparts (U+0020–U+007E), and that decoded text is scanned. An instruction written invisibly in tag characters is matched like a visible one. |
| TR3 | **Tag characters alone are a warning.** A tool result that contains any tag character (U+E0000–E007F) is at least a medium risk, labelled as hidden Unicode tag characters, even when the decoded text matches no pattern. So it gets the existing SECURITY prefix. The result's content isn't changed, because the model needs exact bytes to edit files. |
| TR4 | **ASCII stays fast.** Text that is entirely ASCII is scanned as before, with no normalized copy allocated. Scanning stays linear in the text's length. |
| TR5 | **JSON escapes don't hide a phrase.** Tool results are scanned as JSON text, so a hidden character can arrive as a JSON string escape. Before the TR1 normalization, the scan copy decodes JSON string escapes (`\uXXXX`, including surrogate pairs, plus `\n`, `\t`, `\r`, `\"` and `\\`). So `Ign\u0007ore all previous instructions` or `\u200b` inside the phrase is caught, as are tag characters written as escaped surrogate pairs. |

### Review (TR6–TR7)

Approved end to end (`read_file` and MCP results, errors included). Two small fixes:

| Row | Behaviour |
|---|---|
| TR6 | **Flag emoji aren't a warning.** A well-formed emoji tag sequence isn't counted as hidden tag characters for TR3: U+1F3F4, then one or more of U+E0030–E0039 or U+E0061–E007A, then U+E007F. These are the Scotland, England and Wales flags, and other subdivision flags. Tag characters outside such a sequence still trigger TR3, and the decoded text of every tag sequence is still scanned (TR2). |
| TR7 | **The fast path covers ordinary tool results.** For TR4, the simple JSON escapes `\n`, `\t`, `\r`, `\"`, `\\` and `\/` count as plain. Only a `\u` escape, or a non-ASCII character, makes the scan build a decoded copy. |

**Known limits.**
- `read_file`'s line-number gutter breaks a phrase that is split across lines.
- Only one level of JSON escaping is decoded.
- Look-alike letters from other scripts, such as a Cyrillic `І`, aren't detected.

## Testing notes

- Unit tests in `injection_detector.rs` or `crates/hone-core/tests/`: the variants in TR1, a tag-encoded phrase for TR2, tag characters with a benign payload for TR3, and benign non-Latin text (Persian with ZWNJ, Hindi with ZWJ, Vietnamese) that isn't flagged.
- One agent-level test: a tool result with a split phrase gets the SECURITY prefix in history.
- TR4: a structural test that ASCII input takes the fast path, if it's observable; otherwise a timing guard on a 5 MB ASCII string.

## Out of scope

- Homoglyphs from other scripts, such as a Cyrillic `І`.
- Changing tool-result content.
