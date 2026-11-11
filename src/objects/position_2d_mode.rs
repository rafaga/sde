use super::ProjectedAxis;

/// How `position2DX`/`position2DY` are produced when building a
/// database -- the build-side counterpart of the axis choice
/// [`SdePoint::to_2d`](super::SdePoint::to_2d) makes on the read side.
///
/// EVE's universe geometry (confirmed against real SDE data): the
/// galactic plane is the X-Z plane -- `x` runs map east/west, `z` runs
/// map north/south -- and `y` is the vertical axis (height above/below
/// the plane), which is why it doesn't appear on the flattened map at
/// all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position2DMode {
    /// Trust CCP's own precomputed `position2D` field where present.
    /// Not a projection of the 3D coordinates: it is a hand-adjusted
    /// schematic of the in-game map (a least-squares affine fit of it
    /// against `center` leaves ~10% rms residuals), and it covers
    /// k-space only -- w-space/abyssal/void systems have none.
    /// Default (same behavior the old `force_isometric_position_2d:
    /// false` had).
    #[default]
    Ccp,
    /// Compuphase isometric projection of the 3D coordinates,
    /// [`isometric_projection_2d`](crate::builder::parser::isometric_projection_2d)
    /// with the given collapsed axis (see that function's docs for the
    /// formulas). A true projection, so it covers every system; the
    /// galaxy appears as the classic tilted diamond.
    Isometric(ProjectedAxis),
    /// Orthographic projection that drops the given axis entirely,
    /// keeping the other two components as-is --
    /// `Orthogonal(ProjectedAxis::Y)` is `(x, z)`: the "north-up
    /// top-down" matching the community-canonical map orientation
    /// (east = +x = screen right, north = +z = screen up), with the
    /// vertical `y` left out of the 2D map (still available in
    /// `centerX`/`centerY`/`centerZ`). Same drop semantics
    /// [`SdePoint::to_2d`](super::SdePoint::to_2d) already uses on the read side.
    Orthogonal(ProjectedAxis),
}

impl Position2DMode {
    /// Encodes this mode into the `(forceIsometricPosition2d,
    /// isometricProjectedAxis)` column pair the `sdeFingerprint`
    /// table has always used (no schema change):
    /// `Ccp` -> `(false, "Ccp")`, `Isometric(axis)` ->
    /// `(true, "X"/"Y"/"Z")`, `Orthogonal(axis)` ->
    /// `(true, "OrthogonalX"/"OrthogonalY"/"OrthogonalZ")`. Used by
    /// the write side (`builder::parser::Parser::write_fingerprint`)
    /// and kept next to its counterpart [`Self::from_fingerprint_columns`]
    /// so the two can never drift out of sync.
    pub fn fingerprint_columns(self) -> (bool, String) {
        match self {
            Position2DMode::Ccp => (false, "Ccp".to_string()),
            Position2DMode::Isometric(axis) => (true, format!("{axis:?}")),
            Position2DMode::Orthogonal(axis) => (true, format!("Orthogonal{axis:?}")),
        }
    }

    /// Decodes the column pair [`Self::fingerprint_columns`] encodes.
    /// `(false, _)` maps to `Ccp` (matching the old
    /// `forceIsometricPosition2d = 0` semantics, whatever the axis
    /// text was); `(true, "X"/"Y"/"Z")` maps to `Isometric` (the only
    /// values older versions of this crate ever wrote);
    /// `(true, "OrthogonalX"/"OrthogonalY"/"OrthogonalZ")` maps to
    /// `Orthogonal`. Anything else falls back to
    /// `Isometric(ProjectedAxis::Y)` -- same tolerant-parsing spirit
    /// used throughout for one unexpected value in a column nothing
    /// but this crate writes, rather than failing the whole read.
    pub fn from_fingerprint_columns(force: bool, axis_text: &str) -> Self {
        match (force, axis_text) {
            (false, _) => Position2DMode::Ccp,
            (true, "X") => Position2DMode::Isometric(ProjectedAxis::X),
            (true, "Y") => Position2DMode::Isometric(ProjectedAxis::Y),
            (true, "Z") => Position2DMode::Isometric(ProjectedAxis::Z),
            (true, "OrthogonalX") => Position2DMode::Orthogonal(ProjectedAxis::X),
            (true, "OrthogonalY") => Position2DMode::Orthogonal(ProjectedAxis::Y),
            (true, "OrthogonalZ") => Position2DMode::Orthogonal(ProjectedAxis::Z),
            _ => Position2DMode::Isometric(ProjectedAxis::Y),
        }
    }
}
