//! Member-line parser: `Foo : + bar()` (external) and the inline form
//! found inside `class Foo { … }`. Includes the heuristic that classifies
//! a member as method vs field based on whether the body contains a
//! parenthesized signature.

use crate::ir::{Member, Visibility};

use super::relation::find_arrow_span;

/// `Foo : + bar()` → `Some(("Foo", "+ bar()"))`. Filters lines that look
/// like relations (those have an arrow) so we don't mis-classify
/// `A -- B : associate` as a member.
pub(super) fn split_member_line(raw: &str) -> Option<(String, &str)> {
    if find_arrow_span(raw).is_some() {
        return None;
    }
    let colon = raw.find(':')?;
    let name = raw[..colon].trim();
    if name.is_empty() {
        return None;
    }
    // The id must be a single token (possibly quoted).
    let id = if name.starts_with('"') {
        let close = name[1..].find('"')? + 1;
        if close + 1 != name.len() {
            return None;
        }
        name[1..close].to_string()
    } else if name.contains(char::is_whitespace) {
        return None;
    } else {
        name.to_string()
    };
    Some((id, raw[colon + 1..].trim()))
}

pub(super) fn parse_member(raw: &str, line_no: usize) -> Member {
    let mut s = raw.trim().to_string();
    let mut is_static = false;
    let mut is_abstract = false;
    let mut visibility = Visibility::None;
    // Both `+ {static} foo()` and `{static} + foo()` are valid PlantUML;
    // loop until neither prefix matches so visibility / modifiers can
    // appear in either order.
    loop {
        if let Some((modifier, rest)) = strip_brace_modifier(&s) {
            // `{classifier}` is PUML's spelling for "owned by the class
            // (not the instance)" — same semantics as `{static}`. Treat
            // them identically rather than dropping the modifier.
            if modifier == "static" || modifier == "classifier" {
                is_static = true;
            } else if modifier == "abstract" {
                is_abstract = true;
            }
            s = rest.trim().to_string();
            continue;
        }
        if visibility == Visibility::None {
            if let Some(c) = s.chars().next() {
                if let Some(v) = Visibility::from_char(c) {
                    visibility = v;
                    s = s[c.len_utf8()..].trim_start().to_string();
                    continue;
                }
            }
        }
        break;
    }
    Member {
        visibility,
        is_static,
        is_abstract,
        body: s,
        line: line_no,
    }
}

/// `{static} foo()` → `Some(("static", " foo()"))`. Returns the modifier
/// keyword and the remainder.
fn strip_brace_modifier(s: &str) -> Option<(String, String)> {
    let trimmed = s.trim_start();
    let inner = trimmed.strip_prefix('{')?;
    let close = inner.find('}')?;
    let modifier = inner[..close].trim().to_ascii_lowercase();
    if modifier != "static" && modifier != "abstract" && modifier != "classifier" {
        return None;
    }
    Some((modifier, inner[close + 1..].to_string()))
}

/// Heuristic: a member is a method if it contains a balanced pair of
/// parentheses, a field otherwise.
pub(super) fn is_method_signature(body: &str) -> bool {
    body.contains('(') && body.contains(')')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_member_line_filters_relations() {
        // A line that contains an arrow must not be treated as a
        // member; otherwise `A --> B : ...` would register as a member
        // on `A` named `--> B`.
        assert!(split_member_line("A --> B : owns").is_none());
        assert!(split_member_line("Foo : + bar()").is_some());
    }

    #[test]
    fn split_member_line_requires_single_token_id() {
        // Quoted ids with spaces are accepted; unquoted ids with
        // whitespace are rejected.
        assert!(split_member_line("\"Long Name\" : + foo()").is_some());
        assert!(split_member_line("Two Words : x").is_none());
    }

    #[test]
    fn split_member_line_empty_id_rejected() {
        assert!(split_member_line(" : + foo()").is_none());
    }

    #[test]
    fn parse_member_recognizes_visibility_glyphs() {
        let m = parse_member("+ name: String", 1);
        assert_eq!(m.visibility, Visibility::Public);
        assert_eq!(m.body, "name: String");

        let m = parse_member("- secret", 1);
        assert_eq!(m.visibility, Visibility::Private);

        let m = parse_member("# protected", 1);
        assert_eq!(m.visibility, Visibility::Protected);

        let m = parse_member("~ pkg", 1);
        assert_eq!(m.visibility, Visibility::Package);

        let m = parse_member("plain", 1);
        assert_eq!(m.visibility, Visibility::None);
    }

    #[test]
    fn parse_member_modifier_order_either_way() {
        // `+ {static} foo()` and `{static} + foo()` should both yield a
        // static public method.
        let a = parse_member("+ {static} foo()", 1);
        assert_eq!(a.visibility, Visibility::Public);
        assert!(a.is_static);

        let b = parse_member("{static} + foo()", 1);
        assert_eq!(b.visibility, Visibility::Public);
        assert!(b.is_static);
    }

    #[test]
    fn is_method_signature_method_vs_field() {
        assert!(is_method_signature("getName(): String"));
        assert!(!is_method_signature("name: String"));
        assert!(!is_method_signature("count: int"));
    }

    #[test]
    fn is_method_signature_misclassifies_field_with_parens() {
        // Known heuristic limitation, documented for future work: a
        // field whose body happens to contain `(` and `)` (e.g. a
        // default value expression or a tuple type) gets routed to
        // methods. Pin the current behavior so a future fix has to
        // update this test deliberately.
        assert!(is_method_signature("count: int = default(0)"));
        assert!(is_method_signature("pair: (int, int)"));
    }
}
