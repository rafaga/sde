/// Axis choice for a 3D-to-2D projection -- shared between
/// [`SdePoint::to_2d`](super::SdePoint::to_2d) (explicit, caller-chosen axis) and
/// `builder::parser::isometric_projection_2d` (the isometric map
/// projection). Lives here, not in `builder`, since `SdePoint` needs it
/// on the read side too, and `objects.rs` isn't gated by the `builder`
/// feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectedAxis {
    X,
    /// Default.
    #[default]
    Y,
    Z,
}
