//! Bounding-box and anchor geometry for class diagram entities.
//!
//! Codegen lays nodes out from these geometry estimates and the painter
//! later snaps to them — see `class_geom_filtered` / `note_geom` /
//! `lollipop_geom`. The Rust-side and Typst-side metrics must agree
//! closely enough that an edge anchor lands on the box face the
//! painter draws.

use crate::ir::{Entity, EntityKindData, HideOptions, Member, USymbol, Visibility};
use crate::layout::geometry::Point;

pub(super) const FONT_PT: f64 = 10.0;
/// Bold name glyphs run wider; the markup we emit also includes a
/// stereotype circle (`marker-w` ~ 14pt). 0.62em is conservative for
/// 10pt sans-serif.
pub(super) const NAME_EM: f64 = 0.62;
/// Member rows are regular-weight; visibility glyph adds a small constant.
pub(super) const BODY_EM: f64 = 0.55;
pub(super) const PAD_X_PT: f64 = 0.6 * FONT_PT;
pub(super) const PAD_Y_PT: f64 = 0.3 * FONT_PT;
pub(super) const LINE_HEIGHT_PT: f64 = 1.2 * FONT_PT;
/// Stereotype circle box width allowance (matches painter's `marker-w =
/// 1.4em` when the entity has a circle marker).
pub(super) const MARKER_W_PT: f64 = 1.4 * FONT_PT;

/// Width allowance for the dog-ear fold drawn at the top-right of a
/// note. Codegen has to reserve this in the bbox so the painter's fold
/// triangle doesn't push edge endpoints into the body text.
pub(super) const NOTE_DOG_EAR_PT: f64 = 8.0;

/// Lollipop / interface circle: a small filled disc with the label
/// rendered below. Width = max(disc, label width). Height = disc + gap
/// + label.
pub(super) const LOLLIPOP_DIAMETER_PT: f64 = 14.0;
pub(super) const LOLLIPOP_LABEL_GAP_PT: f64 = 2.0;

pub(super) struct ClassGeom {
    pub size: Point,
    /// Mid-x within the local frame. Used to anchor edge endpoints.
    pub mid_x: f64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Side {
    Top,
    Bot,
    Left,
    Right,
}

impl Side {
    pub(super) fn keyword(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Bot => "bot",
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

pub(super) fn class_geom_filtered(entity: &Entity, hide: &HideOptions) -> ClassGeom {
    if matches!(entity.kind_data, EntityKindData::Note { .. }) {
        return note_geom(entity);
    }
    if entity.usymbol == USymbol::Interface {
        return lollipop_geom(entity);
    }
    let fields = entity.kind_data.fields();
    let methods = entity.kind_data.methods();

    let show_fields = !(hide.fields || hide.members);
    let show_methods = !(hide.methods || hide.members);
    let show_marker = !hide.circle;
    let show_stereo = !hide.stereotype;
    let name_w = name_width_pt_filtered(entity, show_marker, show_stereo);
    let field_w = if show_fields {
        fields.iter().map(member_width_pt).fold(0., f64::max)
    } else {
        0.0
    };
    let method_w = if show_methods {
        methods.iter().map(member_width_pt).fold(0., f64::max)
    } else {
        0.0
    };
    let content_w = name_w.max(field_w).max(method_w);
    let total_w = content_w + 2. * PAD_X_PT;

    let row_h = LINE_HEIGHT_PT + 2. * PAD_Y_PT;
    let name_lines = if show_stereo && entity.stereotype.is_some() {
        2.0
    } else {
        1.0
    };
    let name_h = name_lines * row_h;
    let fields_h = if show_fields {
        fields.len() as f64 * row_h
    } else {
        0.0
    };
    let methods_h = if show_methods {
        methods.len() as f64 * row_h
    } else {
        0.0
    };
    let total_h = name_h + fields_h + methods_h;

    ClassGeom {
        size: Point::new(total_w, total_h),
        mid_x: total_w / 2.0,
    }
}

pub(super) fn lollipop_geom(entity: &Entity) -> ClassGeom {
    let label_w = text_width_pt(&entity.display, BODY_EM);
    let total_w = label_w.max(LOLLIPOP_DIAMETER_PT);
    let total_h = LOLLIPOP_DIAMETER_PT + LOLLIPOP_LABEL_GAP_PT + LINE_HEIGHT_PT;
    ClassGeom {
        size: Point::new(total_w, total_h),
        mid_x: total_w / 2.0,
    }
}

pub(super) fn note_geom(entity: &Entity) -> ClassGeom {
    let body = entity.kind_data.note_body().unwrap_or("");
    let line_widths: Vec<f64> = if body.is_empty() {
        vec![0.0]
    } else {
        body.lines().map(|l| text_width_pt(l, BODY_EM)).collect()
    };
    let max_w = line_widths.iter().cloned().fold(0.0, f64::max);
    let total_w = max_w + 2.0 * PAD_X_PT + NOTE_DOG_EAR_PT;
    let total_h = (line_widths.len() as f64) * LINE_HEIGHT_PT + 2.0 * PAD_Y_PT;
    ClassGeom {
        size: Point::new(total_w, total_h),
        mid_x: total_w / 2.0,
    }
}

pub(super) fn name_width_pt_filtered(
    entity: &Entity,
    show_marker: bool,
    show_stereo: bool,
) -> f64 {
    // Generic parameters render as a small dashed box at the top-right
    // corner of the class — they don't widen the name line.
    let name = entity.display.clone();
    let stereo_w = if show_stereo {
        entity
            .stereotype
            .as_deref()
            .map(|s| text_width_pt(&format!("«{s}»"), BODY_EM))
            .unwrap_or(0.)
    } else {
        0.0
    };
    let title_w = text_width_pt(&name, NAME_EM);
    let marker = if show_marker && entity_marker_letter(entity).is_some() {
        MARKER_W_PT
    } else {
        0.
    };
    title_w.max(stereo_w) + marker
}

/// Marker glyph for this entity, mirroring the painter's chip. Returns
/// `None` for shapes that don't get a chip (lollipops, notes, plain
/// desc shapes). Class-family compartments delegate to
/// [`ClassFamilyKind::marker_letter`].
pub(super) fn entity_marker_letter(entity: &Entity) -> Option<char> {
    match &entity.kind_data {
        EntityKindData::Compartment { kind, .. } => kind.marker_letter(),
        EntityKindData::Note { .. }
        | EntityKindData::Object { .. }
        | EntityKindData::Plain { .. } => None,
    }
}

pub(super) fn member_width_pt(member: &Member) -> f64 {
    let mut s = String::new();
    if member.visibility != Visibility::None {
        s.push_str(member.visibility.glyph());
        s.push(' ');
    }
    s.push_str(&member.body);
    text_width_pt(&s, BODY_EM)
}

fn glyph_width_pt(c: char, em: f64) -> f64 {
    if c.is_ascii() {
        FONT_PT * em
    } else {
        FONT_PT
    }
}

pub(super) fn text_width_pt(s: &str, em: f64) -> f64 {
    s.chars().map(|c| glyph_width_pt(c, em)).sum()
}

pub(super) fn bot_anchor(g: &ClassGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.mid_x, top_left.y + g.size.y)
}

pub(super) fn top_anchor(g: &ClassGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.mid_x, top_left.y)
}

pub(super) fn left_anchor(g: &ClassGeom, top_left: Point) -> Point {
    Point::new(top_left.x, top_left.y + g.size.y / 2.0)
}

pub(super) fn right_anchor(g: &ClassGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.size.x, top_left.y + g.size.y / 2.0)
}

pub(super) fn box_center(g: &ClassGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.size.x / 2.0, top_left.y + g.size.y / 2.0)
}

pub(super) fn anchor_for_side(g: &ClassGeom, top_left: Point, side: Side) -> Point {
    match side {
        Side::Top => top_anchor(g, top_left),
        Side::Bot => bot_anchor(g, top_left),
        Side::Left => left_anchor(g, top_left),
        Side::Right => right_anchor(g, top_left),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ClassFamilyKind, Entity};

    fn plain_entity(name: &str) -> Entity {
        Entity {
            usymbol: USymbol::None,
            id: name.into(),
            display: name.into(),
            stereotype: None,
            stereotype_marker: None,
            fill: None,
            line: 0,
            kind_data: EntityKindData::Compartment {
                kind: ClassFamilyKind::Class,
                generic: None,
                fields: Vec::new(),
                methods: Vec::new(),
            },
        }
    }

    fn push_method(entity: &mut Entity, body: &str) {
        if let EntityKindData::Compartment { methods, .. } = &mut entity.kind_data {
            methods.push(crate::ir::Member {
                visibility: crate::ir::Visibility::Public,
                is_static: false,
                is_abstract: false,
                body: body.into(),
                line: 0,
            });
        }
    }

    #[test]
    fn class_geom_grows_with_stereotype() {
        // A stereotype adds a second header row. Bigger height than a
        // bare class.
        let bare = class_geom_filtered(&plain_entity("A"), &Default::default());
        let mut e = plain_entity("A");
        e.stereotype = Some("Service".into());
        let with_stereo = class_geom_filtered(&e, &Default::default());
        assert!(
            with_stereo.size.y > bare.size.y,
            "stereotype adds a header row; got bare={:?} stereo={:?}",
            bare.size,
            with_stereo.size,
        );
    }

    #[test]
    fn class_geom_drops_methods_when_hidden() {
        let mut e = plain_entity("A");
        push_method(&mut e, "foo()");
        let shown = class_geom_filtered(&e, &Default::default());
        let hidden = class_geom_filtered(
            &e,
            &crate::ir::HideOptions { methods: true, ..Default::default() },
        );
        assert!(
            hidden.size.y < shown.size.y,
            "hide methods should shrink height; shown={:?} hidden={:?}",
            shown.size,
            hidden.size,
        );
    }

    #[test]
    fn anchors_sit_on_the_box_face() {
        let g = ClassGeom {
            size: Point::new(40.0, 30.0),
            mid_x: 20.0,
        };
        let tl = Point::new(10.0, 5.0);
        assert_eq!(top_anchor(&g, tl), Point::new(30.0, 5.0));
        assert_eq!(bot_anchor(&g, tl), Point::new(30.0, 35.0));
        assert_eq!(left_anchor(&g, tl), Point::new(10.0, 20.0));
        assert_eq!(right_anchor(&g, tl), Point::new(50.0, 20.0));
        assert_eq!(box_center(&g, tl), Point::new(30.0, 20.0));
    }
}
