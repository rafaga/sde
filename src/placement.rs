//! Where to put an extra node next to a system, so that it does not sit on
//! top of another node.
//!
//! A map can show a node that is not part of the SDE, such as the end of a
//! wormhole that comes and goes: it has no position of its own, so it is put
//! next to the system it belongs to. [`Placement::place_near`] and
//! [`Placement::place_all`] find that position:
//!
//! * They look for a **free place**: positions around the system are tried
//!   from the closest ring to the farthest one, and the first one where the
//!   node does not touch another is taken. Nothing that is already on the
//!   map is ever moved, and the result depends only on the input, so a node
//!   that is already placed stays where it was when others come or go.
//! * Among the places of a ring, the first tried are the ones **where there is
//!   more room**: away from the nodes around the system.
//! * What counts as touching depends on the size of the node: a circle (the
//!   node of the universe map) or a box (the node of a region map), see
//!   [`Footprint`].
//!
//! It works on plain coordinates and knows nothing of the database, so it
//! serves any map.
//!
//! ```
//! use sde::placement::Placement;
//!
//! let placement = Placement::default();
//! // A system at the origin, and two nodes right at its upper right.
//! let occupied = [[0.0, 0.0], [10.0, -10.0], [14.0, -6.0]];
//! let spot = placement.place_near([0.0, 0.0], &occupied).unwrap();
//!
//! // It is not on top of anything.
//! for other in occupied {
//!     let apart = (spot[0] - other[0]).hypot(spot[1] - other[1]);
//!     assert!(apart >= 16.0 - 1e-6);
//! }
//! ```

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_4, TAU};

/// The size of the nodes of a map, which decides when two of them touch. All
/// the nodes of a map are taken to have the same one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Footprint {
    /// A circle: the node of the universe map is one of radius 4.
    Circle { radius: f64 },
    /// A box, centered on the position of the node: the node of a region map.
    Rect { width: f64, height: f64 },
}

impl Footprint {
    /// Whether this is a size that can be placed: finite and above zero.
    fn is_valid(self) -> bool {
        match self {
            Footprint::Circle { radius } => radius.is_finite() && radius > 0.0,
            Footprint::Rect { width, height } => {
                width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0
            }
        }
    }

    /// The smaller side of the node: what the steps outward are a fraction
    /// of.
    fn size(self) -> f64 {
        match self {
            Footprint::Circle { radius } => 2.0 * radius,
            Footprint::Rect { width, height } => width.min(height),
        }
    }

    /// The farthest apart two nodes can be and still touch, on any axis, with
    /// `gap` between them.
    fn reach(self, gap: f64) -> f64 {
        match self {
            Footprint::Circle { radius } => 2.0 * radius + gap,
            Footprint::Rect { width, height } => width.max(height) + gap,
        }
    }

    /// Whether two nodes centered at `a` and `b` touch or are closer than
    /// `gap`.
    fn touch(self, a: [f64; 2], b: [f64; 2], gap: f64) -> bool {
        // A hair of tolerance so that two nodes at exactly `gap` apart do not
        // touch because of rounding.
        const SLACK: f64 = 1e-9;
        let (dx, dy) = ((a[0] - b[0]).abs(), (a[1] - b[1]).abs());
        match self {
            Footprint::Circle { radius } => {
                let limit = 2.0 * radius + gap;
                dx.hypot(dy) < limit * (1.0 - SLACK)
            }
            Footprint::Rect { width, height } => {
                dx < (width + gap) * (1.0 - SLACK) && dy < (height + gap) * (1.0 - SLACK)
            }
        }
    }

    /// How far from a node, in the direction `angle`, another one has to be
    /// to stop touching it.
    fn clear_distance(self, angle: f64, gap: f64) -> f64 {
        match self {
            Footprint::Circle { radius } => 2.0 * radius + gap,
            Footprint::Rect { width, height } => {
                let (sin, cos) = angle.sin_cos();
                let across = if cos.abs() > 1e-12 {
                    (width + gap) / cos.abs()
                } else {
                    f64::INFINITY
                };
                let along = if sin.abs() > 1e-12 {
                    (height + gap) / sin.abs()
                } else {
                    f64::INFINITY
                };
                across.min(along)
            }
        }
    }
}

/// How an extra node is placed. [`Placement::default`] is for the nodes of
/// the universe map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The size of the nodes.
    pub footprint: Footprint,
    /// The space kept free between two nodes, in map units. With the circle
    /// of the universe map (radius 4) a gap of 8 is a distance of 16 between
    /// centers, the one the projection patch keeps between systems.
    pub gap: f64,
    /// How much farther each ring is from the one before it, as a fraction of
    /// the smaller side of the node.
    pub ring_step: f64,
    /// How many places are tried on each ring, spread over the whole circle.
    pub directions: u32,
    /// How many rings are tried before giving up.
    pub max_rings: u32,
    /// How far around the system the nodes count to tell where there is more
    /// room, as a multiple of the smaller side of the node.
    pub look_radius: f64,
    /// The direction tried first when there is nothing to tell where the room
    /// is (no node around the system, or all around it), in radians. The
    /// maps have `y` pointing down, so the default (-45 degrees) is the upper
    /// right of the screen.
    pub fallback_angle: f64,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            footprint: Footprint::Circle { radius: 4.0 },
            gap: 8.0,
            ring_step: 0.5,
            directions: 24,
            max_rings: 200,
            look_radius: 6.0,
            fallback_angle: -FRAC_PI_4,
        }
    }
}

/// The positions of the nodes, in a grid whose cells are as large as the
/// reach of a node, so the ones that touch a place are in the cells around
/// it.
struct Grid {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<[f64; 2]>>,
}

impl Grid {
    fn new(cell: f64, positions: &[[f64; 2]]) -> Self {
        let mut grid = Self {
            cell,
            cells: HashMap::new(),
        };
        for position in positions {
            grid.insert(*position);
        }
        grid
    }

    fn key(&self, position: [f64; 2]) -> (i64, i64) {
        (
            (position[0] / self.cell).floor() as i64,
            (position[1] / self.cell).floor() as i64,
        )
    }

    fn insert(&mut self, position: [f64; 2]) {
        self.cells
            .entry(self.key(position))
            .or_default()
            .push(position);
    }

    /// The nodes in the cells around `position`.
    fn around(&self, position: [f64; 2]) -> impl Iterator<Item = [f64; 2]> + '_ {
        let (x, y) = self.key(position);
        (-1..=1).flat_map(move |dx| {
            (-1..=1).flat_map(move |dy| {
                self.cells
                    .get(&(x + dx, y + dy))
                    .into_iter()
                    .flatten()
                    .copied()
            })
        })
    }
}

impl Placement {
    /// A place for one node next to the system at `anchor`, away from
    /// everything in `occupied` (the positions of the nodes that are on the
    /// map, the system included). `None` when `anchor` is not a number, the
    /// footprint can't be placed, or no place was free in
    /// [`max_rings`](Self::max_rings) rings.
    pub fn place_near(&self, anchor: [f64; 2], occupied: &[[f64; 2]]) -> Option<[f64; 2]> {
        self.place_all(&[anchor], occupied).pop().flatten()
    }

    /// A place for each of the nodes that go next to the systems at `anchors`,
    /// in the order given: each one is placed away from `occupied` and from
    /// the ones placed before it, so they do not touch each other either. The
    /// answer has one entry per anchor, `None` where [`place_near`](Self::place_near)
    /// would give `None`.
    ///
    /// The place of a node depends on the ones before it and nothing else, so
    /// to keep them where they are when others come or go, give the anchors
    /// in an order that does not change (by the id of what they stand for,
    /// say).
    pub fn place_all(&self, anchors: &[[f64; 2]], occupied: &[[f64; 2]]) -> Vec<Option<[f64; 2]>> {
        if !self.is_valid() {
            return vec![None; anchors.len()];
        }
        let reach = self.footprint.reach(self.gap);
        let mut grid = Grid::new(reach, occupied);
        let mut placed = Vec::with_capacity(anchors.len());
        for anchor in anchors {
            let spot = self.find(*anchor, &grid, occupied);
            if let Some(spot) = spot {
                grid.insert(spot);
            }
            placed.push(spot);
        }
        placed
    }

    fn is_valid(&self) -> bool {
        self.footprint.is_valid()
            && self.gap.is_finite()
            && self.gap >= 0.0
            && self.ring_step.is_finite()
            && self.ring_step > 0.0
            && self.directions > 0
            && self.look_radius.is_finite()
            && self.fallback_angle.is_finite()
    }

    fn find(&self, anchor: [f64; 2], grid: &Grid, occupied: &[[f64; 2]]) -> Option<[f64; 2]> {
        if !anchor[0].is_finite() || !anchor[1].is_finite() {
            return None;
        }
        let preferred = self.roomiest_direction(anchor, grid);
        let step = self.footprint.size() * self.ring_step;
        let directions = self.directions as i64;
        let turn = TAU / directions as f64;
        for ring in 0..=self.max_rings {
            // The directions of a ring, from the preferred one outward on both
            // sides, each pair at the same turn from it.
            for tried in 0..directions {
                let offset = (tried + 1) / 2;
                let sign = if tried % 2 == 1 { 1.0 } else { -1.0 };
                let angle = preferred + sign * offset as f64 * turn;
                let distance = self.footprint.clear_distance(angle, self.gap) * (1.0 + 1e-9)
                    + ring as f64 * step;
                let spot = [
                    anchor[0] + distance * angle.cos(),
                    anchor[1] + distance * angle.sin(),
                ];
                if !self.collides(spot, grid) {
                    return Some(spot);
                }
            }
        }
        // `occupied` is only here so the signature says what the grid holds.
        let _ = occupied;
        None
    }

    /// Whether a node at `spot` touches another.
    fn collides(&self, spot: [f64; 2], grid: &Grid) -> bool {
        grid.around(spot)
            .any(|other| self.footprint.touch(spot, other, self.gap))
    }

    /// The direction, as an angle, where there is more room around `anchor`:
    /// away from the nodes near it, the closer the more they push. The
    /// fallback when they cancel each other or there are none.
    fn roomiest_direction(&self, anchor: [f64; 2], grid: &Grid) -> f64 {
        let radius = self.footprint.size() * self.look_radius;
        let mut push = [0.0, 0.0];
        for other in grid.around(anchor) {
            let (dx, dy) = (anchor[0] - other[0], anchor[1] - other[1]);
            let distance = dx.hypot(dy);
            // The system itself is among the nodes: it has no direction.
            if distance < 1e-9 || distance > radius {
                continue;
            }
            push[0] += dx / (distance * distance);
            push[1] += dy / (distance * distance);
        }
        if push[0].hypot(push[1]) < 1e-12 {
            self.fallback_angle
        } else {
            push[1].atan2(push[0])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apart(a: [f64; 2], b: [f64; 2]) -> f64 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }

    fn boxes() -> Placement {
        Placement {
            footprint: Footprint::Rect {
                width: 94.0,
                height: 39.0,
            },
            gap: 6.0,
            ..Placement::default()
        }
    }

    #[test]
    fn a_node_next_to_a_lone_system_touches_it_without_overlapping() {
        let placement = Placement::default();
        let spot = placement
            .place_near([100.0, 50.0], &[[100.0, 50.0]])
            .unwrap();
        // The closest it can be: radius 4 + gap 8 + radius 4.
        assert!((apart(spot, [100.0, 50.0]) - 16.0).abs() < 1e-6);
        // Toward the fallback direction: the upper right of the screen.
        assert!(spot[0] > 100.0 && spot[1] < 50.0);
    }

    #[test]
    fn it_goes_where_there_is_more_room() {
        let placement = Placement::default();
        // Neighbours of the system to its left and below.
        let occupied = [[0.0, 0.0], [-20.0, 0.0], [0.0, 20.0], [-15.0, 15.0]];
        let spot = placement.place_near([0.0, 0.0], &occupied).unwrap();
        // The room is to the right and above: away from them.
        assert!(spot[0] > 0.0 && spot[1] < 0.0, "{spot:?}");
        for other in occupied {
            assert!(apart(spot, other) >= 16.0 - 1e-6);
        }
    }

    #[test]
    fn a_system_with_no_room_around_gets_a_farther_place() {
        let placement = Placement::default();
        // A ring of nodes right around the system, on all sides.
        let mut occupied = vec![[0.0, 0.0]];
        for step in 0..12 {
            let angle = step as f64 * TAU / 12.0;
            occupied.push([16.0 * angle.cos(), 16.0 * angle.sin()]);
        }
        let spot = placement.place_near([0.0, 0.0], &occupied).unwrap();
        assert!(apart(spot, [0.0, 0.0]) > 16.0 + 1.0, "{spot:?}");
        for other in &occupied {
            assert!(apart(spot, *other) >= 16.0 - 1e-6, "touches {other:?}");
        }
    }

    #[test]
    fn boxes_need_less_room_above_than_to_the_side() {
        let placement = boxes();
        // A neighbour to each side of the system.
        let occupied = [[0.0, 0.0], [-100.0, 0.0], [100.0, 0.0]];
        let spot = placement.place_near([0.0, 0.0], &occupied).unwrap();
        // Boxes of 94 x 39 with a gap of 6 clear each other at 45 vertically,
        // far less than the 100 they need horizontally: that is what is
        // between the node and the system, not a circle of 100.
        assert!((spot[1].abs() - 45.0).abs() < 1e-3, "{spot:?}");
        assert!(apart(spot, [0.0, 0.0]) < 100.0, "{spot:?}");
        for other in occupied {
            let dx = (spot[0] - other[0]).abs();
            let dy = (spot[1] - other[1]).abs();
            assert!(dx >= 100.0 - 1e-6 || dy >= 45.0 - 1e-6, "touches {other:?}");
        }
    }

    #[test]
    fn several_nodes_for_one_system_do_not_touch_each_other() {
        let placement = Placement::default();
        let anchors = [[0.0, 0.0]; 6];
        let placed = placement.place_all(&anchors, &[[0.0, 0.0]]);
        let spots: Vec<[f64; 2]> = placed.into_iter().map(Option::unwrap).collect();
        for (index, first) in spots.iter().enumerate() {
            assert!(apart(*first, [0.0, 0.0]) >= 16.0 - 1e-6);
            for second in &spots[index + 1..] {
                assert!(
                    apart(*first, *second) >= 16.0 - 1e-6,
                    "{first:?} {second:?}"
                );
            }
        }
    }

    #[test]
    fn the_same_input_gives_the_same_places_and_earlier_ones_do_not_move() {
        let placement = Placement::default();
        let occupied = [[0.0, 0.0], [30.0, 5.0], [-10.0, 25.0]];
        let anchors = [[0.0, 0.0], [30.0, 5.0], [-10.0, 25.0], [0.0, 0.0]];
        let all = placement.place_all(&anchors, &occupied);
        assert_eq!(all, placement.place_all(&anchors, &occupied));
        // Asking for fewer gives the same places for those: the ones before
        // do not depend on the ones after.
        let fewer = placement.place_all(&anchors[..2], &occupied);
        assert_eq!(fewer, all[..2]);
    }

    #[test]
    fn nothing_that_is_there_is_moved_and_the_answer_has_one_entry_per_anchor() {
        let placement = Placement::default();
        let placed = placement.place_all(&[[0.0, 0.0], [500.0, 500.0]], &[[0.0, 0.0]]);
        assert_eq!(placed.len(), 2);
        // The second system is not among the occupied ones: still gets a place.
        assert!(placed.iter().all(Option::is_some));
    }

    #[test]
    fn a_place_that_can_not_be_found_is_none() {
        // Nodes all around the system, so the single try (one direction, no
        // farther rings) lands on one of them.
        let mut occupied = vec![[0.0, 0.0]];
        for step in 0..12 {
            let angle = step as f64 * TAU / 12.0;
            occupied.push([16.0 * angle.cos(), 16.0 * angle.sin()]);
        }
        let blocked = Placement {
            max_rings: 0,
            directions: 1,
            ..Placement::default()
        };
        assert_eq!(blocked.place_near([0.0, 0.0], &occupied), None);
        // With the rings and directions it is given by default, there is one.
        assert!(
            Placement::default()
                .place_near([0.0, 0.0], &occupied)
                .is_some()
        );
    }

    #[test]
    fn what_can_not_be_placed_gives_none_instead_of_looping() {
        let anchors = [[0.0, 0.0]];
        for bad in [
            Placement {
                footprint: Footprint::Circle { radius: 0.0 },
                ..Placement::default()
            },
            Placement {
                footprint: Footprint::Rect {
                    width: f64::NAN,
                    height: 10.0,
                },
                ..Placement::default()
            },
            Placement {
                directions: 0,
                ..Placement::default()
            },
            Placement {
                ring_step: 0.0,
                ..Placement::default()
            },
            Placement {
                gap: -1.0,
                ..Placement::default()
            },
        ] {
            assert_eq!(bad.place_all(&anchors, &[]), vec![None]);
        }
        assert_eq!(Placement::default().place_near([f64::NAN, 0.0], &[]), None);
    }
}
