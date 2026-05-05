//! Line scanner. Extracts `@start<kind> … @end<kind>` blocks and tracks the
//! source line of each opening tag plus the kind suffix (`uml`, `mindmap`,
//! `wbs`, `json`, `salt`, `gantt`, `ditaa`, …) so the dispatcher can trust
//! the author's intent rather than guess from body content.

/// One `@start<kind> … @end<kind>` block lifted out of a source file.
#[derive(Clone, Debug)]
pub struct UmlBlock {
    /// Line number (1-based) of the opening tag.
    pub start_line: usize,
    /// The suffix on the opening tag — `"uml"` for `@startuml`,
    /// `"mindmap"` for `@startmindmap`, and so on. Lower-cased.
    /// Empty when the file is fragment mode (no tags at all).
    pub kind_tag: String,
    /// Optional name following the tag, e.g. `@startuml MyDiagram`.
    pub name: Option<String>,
    /// Body lines between the opening and closing tags, exclusive.
    pub body: Vec<BodyLine>,
}

#[derive(Clone, Debug)]
pub struct BodyLine {
    /// 1-based source line number — preserved so diagnostics can point
    /// back into the original `.puml` even after preprocessing reorders
    /// content via `!include`.
    pub line: usize,
    pub text: String,
}

/// Find every `@start<kind> … @end<kind>` block in `source` and return them
/// in order. Tags must appear at the start of a line (after optional
/// whitespace). When no opening tag is present, the entire input is treated
/// as a single nameless block with an empty `kind_tag` — matches PlantUML's
/// behavior for fragment `.puml` files.
pub fn extract_uml_blocks(source: &str) -> Vec<UmlBlock> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    let mut found_any_tag = false;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if let Some((kind, rest)) = strip_start_tag(trimmed) {
            found_any_tag = true;
            let name = rest.trim();
            let name = if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            };
            let start_line = i + 1;
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() {
                let t = lines[i].trim_start();
                if is_end_tag(t, &kind) {
                    i += 1;
                    break;
                }
                body.push(BodyLine {
                    line: i + 1,
                    text: lines[i].to_string(),
                });
                i += 1;
            }
            blocks.push(UmlBlock {
                start_line,
                kind_tag: kind,
                name,
                body,
            });
        } else {
            i += 1;
        }
    }

    if !found_any_tag {
        let body = source
            .lines()
            .enumerate()
            .map(|(idx, text)| BodyLine {
                line: idx + 1,
                text: text.to_string(),
            })
            .collect();
        blocks.push(UmlBlock {
            start_line: 1,
            kind_tag: String::new(),
            name: None,
            body,
        });
    }

    blocks
}

/// If `line` begins with `@start<kind>`, return `(kind, rest)`. The kind is
/// the lower-cased letters/digits immediately after `@start`; `rest` is
/// everything past it (the optional diagram name).
fn strip_start_tag(line: &str) -> Option<(String, &str)> {
    let rest = line.strip_prefix("@start")?;
    let split = rest
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(rest.len());
    let (kind, tail) = rest.split_at(split);
    if kind.is_empty() {
        return None;
    }
    Some((kind.to_ascii_lowercase(), tail))
}

fn is_end_tag(line: &str, kind: &str) -> bool {
    // Be tolerant: `@enduml` closes any block (matches PlantUML's CLI).
    let Some(rest) = line.strip_prefix("@end") else {
        return false;
    };
    let suffix = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>();
    suffix.eq_ignore_ascii_case(kind) || suffix.eq_ignore_ascii_case("uml") || suffix.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_single_uml_block() {
        let blocks = extract_uml_blocks("@startuml\nA -> B\n@enduml\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind_tag, "uml");
        assert_eq!(blocks[0].body.len(), 1);
        assert_eq!(blocks[0].body[0].text, "A -> B");
    }

    #[test]
    fn extracts_named_block() {
        let blocks = extract_uml_blocks("@startuml MyDiagram\nA -> B\n@enduml\n");
        assert_eq!(blocks[0].name.as_deref(), Some("MyDiagram"));
    }

    #[test]
    fn recognizes_mindmap_and_wbs() {
        let src = "@startmindmap\n* root\n@endmindmap\n@startwbs\n* root\n@endwbs\n";
        let blocks = extract_uml_blocks(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind_tag, "mindmap");
        assert_eq!(blocks[1].kind_tag, "wbs");
    }

    #[test]
    fn fragment_without_tags_becomes_one_block() {
        let blocks = extract_uml_blocks("A -> B\nC -> D\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind_tag, "");
        assert_eq!(blocks[0].body.len(), 2);
    }

    #[test]
    fn closing_uml_closes_any_block() {
        let blocks = extract_uml_blocks("@startmindmap\n* root\n@enduml\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind_tag, "mindmap");
        assert_eq!(blocks[0].body.len(), 1);
    }
}
