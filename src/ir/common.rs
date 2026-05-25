//! Shared IR types used across more than one diagram family.

/// Top-level layout flow. PlantUML's `left to right direction` flips
/// the Sugiyama orientation; everything else (`top to bottom`,
/// default) keeps TB.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum LayoutDirection {
    #[default]
    TopToBottom,
    LeftToRight,
}

#[derive(Clone, Debug)]
pub struct Skinparam {
    pub key: String,
    pub value: String,
    pub line: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NotePosition {
    Over,
    LeftOf,
    RightOf,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LineStyle {
    Solid,
    Dashed,
    Dotted,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}
