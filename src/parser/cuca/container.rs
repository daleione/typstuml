//! Container declarations: `package`, `namespace`, `together`, `folder`,
//! `frame`, `node`, `cloud`.

use crate::ir::USymbol;

use super::entity::{pop_trailing_color, pop_trailing_generic, pop_trailing_stereotype};
use super::util::{strip_leading_quoted, strip_prefix_keyword};

/// Parser-side description of a container declaration. `together: true`
/// is PlantUML's anonymous grouping block — borderless, label-less.
pub(super) struct ContainerOpen {
    pub usymbol: USymbol,
    pub together: bool,
    pub label: String,
    pub stereotype: Option<String>,
}

/// If `raw` opens a container block (`package "Foo" {`,
/// `namespace foo.bar {`, `together {`, `folder X {`, …), return the
/// shape, the label (empty for `together`), and an optional `<<stereo>>`
/// found between the name and the `#color`. Returns `None` otherwise.
///
/// The 17 container-capable shapes match PlantUML's
/// `CommandPackageWithUSymbol.getRegexConcat:76` whitelist (see
/// `docs/cuca-diagram-design.md` §3.1).
pub(super) fn parse_container_open(raw: &str) -> Option<ContainerOpen> {
    // `namespace` shares `USymbol::Package` with `package` — they paint
    // identically in v1; M5+ may revisit if PlantUML adds a distinct
    // skinparam path.
    const KW: &[(&str, USymbol, bool)] = &[
        ("package", USymbol::Package, false),
        ("namespace", USymbol::Package, false),
        ("together", USymbol::None, true),
        // PlantUML CommandPackageWithUSymbol whitelist:
        ("rectangle", USymbol::Rectangle, false),
        ("hexagon", USymbol::Hexagon, false),
        ("node", USymbol::Node, false),
        ("artifact", USymbol::Artifact, false),
        ("folder", USymbol::Folder, false),
        ("file", USymbol::File, false),
        ("frame", USymbol::Frame, false),
        ("cloud", USymbol::Cloud, false),
        ("action", USymbol::Action, false),
        ("process", USymbol::Process, false),
        ("database", USymbol::Database, false),
        ("storage", USymbol::Storage, false),
        ("component", USymbol::Component, false),
        ("card", USymbol::Card, false),
        ("queue", USymbol::Queue, false),
        ("stack", USymbol::Stack, false),
    ];
    for (kw, usymbol, together) in KW {
        let Some(rest) = strip_prefix_keyword(raw, kw) else {
            continue;
        };
        let rest = rest.trim_end();
        if !rest.ends_with('{') {
            continue;
        }
        let body = rest[..rest.len() - 1].trim();
        // Order matches entity-decl: trailing color, then stereotype, then
        // generic — generic is rare but legal on a `package`. The label is
        // what's left after stripping all three; quoted form unwraps quotes.
        let mut working = body.to_string();
        let _color = pop_trailing_color(&mut working);
        let stereotype = pop_trailing_stereotype(&mut working);
        let _generic = pop_trailing_generic(&mut working);
        let label_raw = working.trim();
        // `together` doesn't take a name; everything else does.
        let label = if *together {
            String::new()
        } else if let Some((quoted, _)) = strip_leading_quoted(label_raw) {
            quoted
        } else {
            label_raw.split_whitespace().next().unwrap_or("").to_string()
        };
        return Some(ContainerOpen {
            usymbol: *usymbol,
            together: *together,
            label,
            stereotype,
        });
    }
    None
}
