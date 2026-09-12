//! Geometry-only graph backend shared by diagram-specific code generators.
//! This layer must not depend on parsers or semantic diagram IR.
pub(super) mod compound;
pub(super) mod geom;
pub(super) mod labels;
pub(super) mod placement;
pub(super) mod route;
pub(super) mod routing;
