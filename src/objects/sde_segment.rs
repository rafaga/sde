use rstar::{AABB, PointDistance, RTreeObject};

/// Line segment between two solar systems (a stargate connection, or an
/// edge on the abstract map). A type owned by `sde`, replaces the
/// `SdeSegment` that used to come from `egui-map`.
///
/// Implements `rstar`'s [`RTreeObject`]/[`PointDistance`] -- not
/// because [`crate::SdeManager::get_connections`]/`get_abstract_connections`
/// build an `rstar::RTree` internally (they return a plain
/// `HashMap<(usize, usize), SdeSegment>`, keyed by `id`, the same
/// shape every other getter here uses), but so that anyone who wants
/// spatial queries ("which connections fall within this area of the
/// map", "which connection is closest to where the user clicked") can
/// build one directly from the map's values
/// (`rstar::RTree::bulk_load(map.into_values().collect())`) without
/// this crate forcing that cost on callers who don't need it.
///
/// `id` is the pair of system ids it connects -- no longer an arbitrary
/// `Rc<str>` like in the old `egui-map` integration. This `(usize,
/// usize)` is exactly the shape `egui_map::SdeSegment.id` would need
/// after the targeted change already designed for that crate (`Rc<str>`
/// -> tuple) -- the only piece of `egui-map` that needs touching to
/// interoperate, without redesigning anything else there.
///
/// `point1`/`point2` are `[f64; 2]`, matching [`SdePoint`](super::SdePoint)'s coordinate
/// type -- both represent the same kind of value (a system's 2D
/// position), so there's no reason for one to be `f64` and the other
/// `f32`. This is a change from the type's initial version, which kept
/// `[f32; 2]` to match `egui_map::RawPoint.components` 1:1 without any
/// conversion; that 1:1 match no longer holds -- an `egui-map` consumer
/// building a `RawPoint` from this now needs an explicit narrowing
/// (`point1[0] as f32`, etc.), same as it already would from
/// `SdePoint`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SdeSegment {
    pub id: (usize, usize),
    pub point1: [f64; 2],
    pub point2: [f64; 2],
}

impl RTreeObject for SdeSegment {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(self.point1, self.point2)
    }
}

impl PointDistance for SdeSegment {
    /// Squared distance from `point` to the closest point *on the
    /// segment* (not just its bounding box) -- delegates to
    /// `rstar::primitives::Line`'s own implementation (clamped
    /// projection onto the segment) instead of re-deriving that
    /// point-to-segment math by hand, since getting the clamping at
    /// the endpoints subtly wrong is an easy mistake to make. Verified
    /// against a real compiler: a point beyond either endpoint
    /// correctly measures to that endpoint, not to the infinite line
    /// through the segment.
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        rstar::primitives::Line::new(self.point1, self.point2).distance_2(point)
    }
}

/// Converts an `rstar::RTree<SdeSegment>` into a list of references to
/// its segments, **without cloning** -- for anyone who prefers to
/// iterate/consume it as a plain list instead of using `rstar`'s native
/// spatial queries (`locate_in_envelope_intersecting`,
/// `nearest_neighbor`, etc.). Doesn't require the tree to come from
/// this crate specifically -- useful with any `RTree<SdeSegment>`,
/// including one a caller built themselves (e.g. from
/// [`crate::SdeManager::get_connections`]/`get_abstract_connections`'s
/// `HashMap` values, for anyone who does want the spatial-query
/// capabilities an `RTree` provides).
///
/// `RTree` already exposes a plain `iter()` over every element, so this
/// is a direct wrapper -- no extra work needed to get a plain list out
/// of it.
pub fn map_segments_to_vec(tree: &rstar::RTree<SdeSegment>) -> Vec<&SdeSegment> {
    tree.iter().collect()
}
