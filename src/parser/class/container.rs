//! Container declarations: `package`, `namespace`, `together`, `folder`,
//! `frame`, `node`, `cloud`.

use crate::ir::ContainerKind;

use super::entity::{pop_trailing_color, pop_trailing_generic, pop_trailing_stereotype};
use super::util::{strip_leading_quoted, strip_prefix_keyword};

/// If `raw` opens a container block (`package "Foo" {`,
/// `namespace foo.bar {`, `together {`, `folder X {`, …), return the
/// kind, the label (empty for `together`), and an optional `<<stereo>>`
/// found between the name and the `#color`. Returns `None` otherwise.
pub(super) fn parse_container_open(
    raw: &str,
) -> Option<(ContainerKind, String, Option<String>)> {
    const KW: &[(&str, ContainerKind)] = &[
        ("package", ContainerKind::Package),
        ("namespace", ContainerKind::Namespace),
        ("together", ContainerKind::Together),
        ("folder", ContainerKind::Folder),
        ("frame", ContainerKind::Frame),
        ("node", ContainerKind::Node),
        ("cloud", ContainerKind::Cloud),
    ];
    for (kw, kind) in KW {
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
        let label = if matches!(kind, ContainerKind::Together) {
            String::new()
        } else if let Some((quoted, _)) = strip_leading_quoted(label_raw) {
            quoted
        } else {
            label_raw.split_whitespace().next().unwrap_or("").to_string()
        };
        return Some((*kind, label, stereotype));
    }
    None
}
