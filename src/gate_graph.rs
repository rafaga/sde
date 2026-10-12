//! Route planning over the stargate network.
//!
//! [`GateGraph`] models the universe the way a pilot experiences it: the
//! nodes are **stargates**, not solar systems. That matters because the
//! cost of crossing a system depends on *which* gate you arrived through
//! and *which* gate you leave by -- the distance between the two is what
//! the pilot actually flies. A graph of systems cannot represent that,
//! because two routes that reach the same system through different gates
//! are not equivalent.
//!
//! Two kinds of edges connect the gates:
//!
//! - **Legs**: between two gates of the same system. A leg is as long as
//!   the straight-line distance between the gates (see [`Gate::position`]
//!   for the units).
//! - **Jumps**: from a gate to the gate on the other side of it
//!   ([`Gate::destination_id`]). The pilot does not control this part --
//!   it is the time the server takes to move the ship between instances --
//!   so every jump is the same.
//!
//! What the "best" route is depends on what the caller wants to save, and
//! that is the caller's choice, made with a [`Priority`]:
//!
//! - [`Priority::FewestJumps`]: the route with the fewest jumps; among the
//!   routes that tie, the one with the shortest flight.
//! - [`Priority::ShortestFlight`]: the route with the shortest total
//!   distance flown inside systems; among the routes that tie, the one
//!   with the fewest jumps.
//! - [`Priority::Balanced`]: a single number that trades one for the
//!   other. See [`RouteCost`].
//!
//! [`GateGraph::route`] finds the route with Dijkstra's algorithm.
//!
//! The graph is built in memory from `mapSystemGates` (one query, a few
//! tens of milliseconds for the whole universe) and the leg distances are
//! computed on demand while searching, so nothing derived is stored in the
//! database.

use crate::{Error, SdeManager};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::ops::Add;

/// Meters in one astronomical unit. Handy to express a [`RouteCost`] in
/// AU: `Priority::balanced(2.0 * METERS_PER_AU, 0.0)`.
pub const METERS_PER_AU: f64 = 149_597_870_700.0;

/// A stargate, as stored in `mapSystemGates`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gate {
    /// The gate's id (`mapSystemGates.systemGateId`).
    pub id: usize,
    /// The solar system the gate is in.
    pub system_id: usize,
    /// The gate on the other side of this one.
    pub destination_id: usize,
    /// The solar system on the other side of this gate.
    pub destination_system_id: usize,
    /// Position of the gate inside its system, exactly as the SDE stores
    /// it: `[x, y, z]`, unscaled (the SDE uses meters, and
    /// [`SdeManager::factor`] is **not** applied). Positions of gates in
    /// different systems live in different frames of reference and must
    /// never be compared with each other.
    pub position: [f64; 3],
}

impl Gate {
    /// Straight-line distance from this gate to `point`, in the same
    /// units as [`Gate::position`].
    pub fn distance_to(&self, point: [f64; 3]) -> f64 {
        let dx = self.position[0] - point[0];
        let dy = self.position[1] - point[1];
        let dz = self.position[2] - point[2];
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// How a [`Priority::Balanced`] route is priced:
///
/// ```text
/// cost = sum(leg distances) + leg_cost * legs + jump_cost * jumps
/// ```
///
/// `jump_cost` is the knob that balances "fewer jumps" against "less
/// distance": it is the distance that one jump is considered to be worth.
/// A value of zero ignores jumps entirely. For a real fewest-jumps search
/// use [`Priority::FewestJumps`] instead of a huge number here, which
/// would swallow the distances in floating-point rounding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteCost {
    /// What one stargate jump costs, in the same units as the distances
    /// (meters by default). It is the distance that one jump is
    /// considered to be worth.
    pub jump_cost: f64,
    /// A fixed cost added to every leg (every flight from one point of a
    /// system to another), regardless of its length. It penalises routes
    /// made of many short legs, which is where the alignment time of a
    /// real warp goes. `0.0` by default.
    pub leg_cost: f64,
}

impl RouteCost {
    /// Builds a cost model from a jump cost and a per-leg cost.
    pub fn new(jump_cost: f64, leg_cost: f64) -> Self {
        RouteCost {
            jump_cost,
            leg_cost,
        }
    }

    /// Checks that both numbers can be used as edge weights: finite and
    /// not negative. Dijkstra's algorithm is only correct for
    /// non-negative weights.
    fn validate(&self) -> Result<(), Error> {
        for (name, value) in [("jump_cost", self.jump_cost), ("leg_cost", self.leg_cost)] {
            if !value.is_finite() || value < 0.0 {
                return Err(Error::route(format!(
                    "{name} must be a finite, non-negative number (got {value})"
                )));
            }
        }
        Ok(())
    }
}

impl Default for RouteCost {
    /// One jump is worth one astronomical unit of flight, and a leg has
    /// no fixed cost. This is only a starting point: there is no single
    /// right value, it depends on the ship and on how much a jump hurts.
    fn default() -> Self {
        RouteCost {
            jump_cost: METERS_PER_AU,
            leg_cost: 0.0,
        }
    }
}

/// What a route should save: jumps, flight distance, or a mix of both.
/// The caller chooses; there is no universally best route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Priority {
    /// The fewest stargate jumps. Routes with the same number of jumps
    /// are ordered by their flight distance, shortest first.
    FewestJumps,
    /// The shortest total distance flown inside systems. Routes with the
    /// same distance are ordered by their number of jumps, fewest first.
    ShortestFlight,
    /// Jumps and distance traded against each other with a single
    /// number. See [`RouteCost`].
    Balanced(RouteCost),
}

impl Priority {
    /// Shorthand for `Priority::Balanced(RouteCost::new(jump_cost, leg_cost))`.
    pub fn balanced(jump_cost: f64, leg_cost: f64) -> Self {
        Priority::Balanced(RouteCost::new(jump_cost, leg_cost))
    }

    /// Fails if a [`Priority::Balanced`] cost has a number that cannot be
    /// used as an edge weight.
    fn validate(&self) -> Result<(), Error> {
        match self {
            Priority::Balanced(cost) => cost.validate(),
            _ => Ok(()),
        }
    }

    /// The price of every kind of movement under this priority.
    fn pricing(&self) -> Pricing {
        match self {
            Priority::FewestJumps => Pricing {
                jump: Cost::new(1.0, 0.0),
                leg_fixed: Cost::ZERO,
                per_meter: Cost::new(0.0, 1.0),
            },
            Priority::ShortestFlight => Pricing {
                jump: Cost::new(0.0, 1.0),
                leg_fixed: Cost::ZERO,
                per_meter: Cost::new(1.0, 0.0),
            },
            Priority::Balanced(cost) => Pricing {
                jump: Cost::new(cost.jump_cost, 0.0),
                leg_fixed: Cost::new(cost.leg_cost, 0.0),
                per_meter: Cost::new(1.0, 0.0),
            },
        }
    }
}

impl Default for Priority {
    /// [`Priority::Balanced`] with the default [`RouteCost`].
    fn default() -> Self {
        Priority::Balanced(RouteCost::default())
    }
}

/// A cost made of two numbers compared in order: the `primary` one first,
/// and the `secondary` one only to break a tie. Dijkstra's algorithm works
/// the same with this as with a single number because both parts only ever
/// grow when edges are added.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
struct Cost {
    primary: f64,
    secondary: f64,
}

impl Cost {
    const ZERO: Cost = Cost {
        primary: 0.0,
        secondary: 0.0,
    };
    const INFINITY: Cost = Cost {
        primary: f64::INFINITY,
        secondary: f64::INFINITY,
    };

    fn new(primary: f64, secondary: f64) -> Self {
        Cost { primary, secondary }
    }

    fn scaled(self, factor: f64) -> Self {
        Cost::new(self.primary * factor, self.secondary * factor)
    }
}

impl Add for Cost {
    type Output = Cost;

    fn add(self, other: Cost) -> Cost {
        Cost::new(
            self.primary + other.primary,
            self.secondary + other.secondary,
        )
    }
}

/// What each kind of movement adds to a [`Cost`]; built from a
/// [`Priority`].
#[derive(Debug, Clone, Copy)]
struct Pricing {
    /// One stargate jump.
    jump: Cost,
    /// Added to every leg, whatever its length.
    leg_fixed: Cost,
    /// Added to a leg for every meter it is long.
    per_meter: Cost,
}

impl Pricing {
    /// The cost of a leg of the given length.
    fn leg(&self, distance: f64) -> Cost {
        self.per_meter.scaled(distance) + self.leg_fixed
    }
}

/// Where a route starts or ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Endpoint {
    /// A stargate by id. The route starts (or ends) exactly at the gate,
    /// so no leg is needed to reach it.
    Gate(usize),
    /// A solar system, without a position inside it. Which gate you
    /// enter or leave by is free, and getting to it costs nothing.
    System(usize),
    /// A specific point of a solar system (a station, a structure, a
    /// celestial). `position` uses the same frame and units as
    /// [`Gate::position`]. Flying to or from it is a leg.
    Point {
        /// The solar system the point is in.
        system_id: usize,
        /// `[x, y, z]` inside that system.
        position: [f64; 3],
    },
}

impl Endpoint {
    /// The solar system the endpoint refers to, when it names one directly.
    /// For [`Endpoint::Gate`] the system depends on the graph.
    fn system_id(&self) -> Option<usize> {
        match self {
            Endpoint::Gate(_) => None,
            Endpoint::System(id) => Some(*id),
            Endpoint::Point { system_id, .. } => Some(*system_id),
        }
    }
}

/// The result of [`GateGraph::route`], with the breakdown of its cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// Every gate the route passes through, in order: the gate used to
    /// leave each system and the gate you arrive through in the next one.
    /// Empty when the route does not need any gate (both endpoints are in
    /// the same system and none of them is a gate).
    pub gates: Vec<usize>,
    /// Number of stargate jumps.
    pub jumps: usize,
    /// Number of legs (flights inside a system), including the ones from
    /// and to a [`Endpoint::Point`].
    pub legs: usize,
    /// Total length of all the legs, in the units of [`Gate::position`].
    pub distance: f64,
    /// The quantity that the search minimised first, which depends on the
    /// [`Priority`]: the number of jumps for [`Priority::FewestJumps`],
    /// the distance for [`Priority::ShortestFlight`], and
    /// `distance + legs * leg_cost + jumps * jump_cost` for
    /// [`Priority::Balanced`].
    pub cost: f64,
}

/// A search state in the priority queue, ordered so that the *cheapest*
/// one comes out first from [`BinaryHeap`] (which is a max-heap).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Visit {
    cost: Cost,
    node: usize,
}

impl Eq for Visit {}

impl Ord for Visit {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .primary
            .total_cmp(&self.cost.primary)
            .then_with(|| other.cost.secondary.total_cmp(&self.cost.secondary))
            .then_with(|| self.node.cmp(&other.node))
    }
}

impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The stargate network, ready for route queries. See the
/// [module documentation](self).
#[derive(Debug, Clone, Default)]
pub struct GateGraph {
    /// Every gate, indexed by its position in this vector (the node id
    /// used during the search).
    gates: Vec<Gate>,
    /// Gate id -> index in `gates`.
    index: HashMap<usize, usize>,
    /// System id -> indexes in `gates` of the gates it has.
    by_system: HashMap<usize, Vec<usize>>,
}

impl GateGraph {
    /// Builds a graph from a list of gates. If two gates share an id the
    /// first one wins. Gates whose [`Gate::destination_id`] is not in the
    /// list simply have no jump edge (their other side is out of scope).
    pub fn from_gates(gates: Vec<Gate>) -> Self {
        let mut graph = GateGraph::default();
        for gate in gates {
            if graph.index.contains_key(&gate.id) {
                continue;
            }
            let node = graph.gates.len();
            graph.index.insert(gate.id, node);
            graph
                .by_system
                .entry(gate.system_id)
                .or_default()
                .push(node);
            graph.gates.push(gate);
        }
        graph
    }

    /// Reads every row of `mapSystemGates` from the database `manager`
    /// points at and builds the graph. Positions are taken as stored, so
    /// [`SdeManager::factor`] and `invert_coordinates` do not apply and
    /// distances come out in the SDE's own units (meters).
    #[tracing::instrument(skip(manager))]
    pub fn load(manager: &SdeManager<'_>) -> Result<Self, Error> {
        let connection = manager.get_standart_connection()?;
        let mut statement = connection.prepare(
            "SELECT systemGateId, solarSystemId, destinationGateId, destinationSystemId, \
             positionX, positionY, positionZ FROM mapSystemGates;",
        )?;
        let mut rows = statement.query([])?;
        let mut gates = Vec::new();
        while let Some(row) = rows.next()? {
            gates.push(Gate {
                id: row.get::<usize, i64>(0)? as usize,
                system_id: row.get::<usize, i64>(1)? as usize,
                destination_id: row.get::<usize, i64>(2)? as usize,
                destination_system_id: row.get::<usize, i64>(3)? as usize,
                position: [
                    row.get::<usize, f64>(4)?,
                    row.get::<usize, f64>(5)?,
                    row.get::<usize, f64>(6)?,
                ],
            });
        }
        Ok(GateGraph::from_gates(gates))
    }

    /// Number of gates in the graph.
    pub fn gate_count(&self) -> usize {
        self.gates.len()
    }

    /// Number of solar systems that have at least one gate.
    pub fn system_count(&self) -> usize {
        self.by_system.len()
    }

    /// The gate with the given id, if the graph has it.
    pub fn gate(&self, id: usize) -> Option<&Gate> {
        self.index.get(&id).map(|node| &self.gates[*node])
    }

    /// The gates of a solar system, in no particular order. Empty when
    /// the system is unknown or has no gates.
    pub fn gates_in_system(&self, system_id: usize) -> impl Iterator<Item = &Gate> {
        self.by_system
            .get(&system_id)
            .into_iter()
            .flatten()
            .map(|node| &self.gates[*node])
    }

    /// The best route from `from` to `to` for the given `priority`.
    ///
    /// Returns `Ok(None)` when both endpoints exist but no route connects
    /// them, and an error when an endpoint does not exist in the graph or
    /// a [`Priority::Balanced`] cost has a negative or non-finite number.
    ///
    /// Asking for a route between two endpoints in the same system, none
    /// of them a gate, never uses a gate: it is a single leg (or nothing
    /// at all if neither endpoint has a position).
    #[tracing::instrument(skip(self))]
    pub fn route(
        &self,
        from: Endpoint,
        to: Endpoint,
        priority: &Priority,
    ) -> Result<Option<Route>, Error> {
        priority.validate()?;
        self.check_endpoint(&from)?;
        self.check_endpoint(&to)?;
        let pricing = priority.pricing();

        if let (Some(a), Some(b)) = (from.system_id(), to.system_id())
            && a == b
        {
            return Ok(Some(Self::direct_route(&from, &to, &pricing)));
        }

        let mut best_cost = vec![Cost::INFINITY; self.gates.len()];
        let mut previous: Vec<Option<usize>> = vec![None; self.gates.len()];
        let mut queue = BinaryHeap::new();

        for (node, start) in self.entry_costs(&from, &pricing) {
            if start < best_cost[node] {
                best_cost[node] = start;
                queue.push(Visit { cost: start, node });
            }
        }

        let mut best_total = Cost::INFINITY;
        let mut best_node = None;
        while let Some(Visit { cost: spent, node }) = queue.pop() {
            if spent >= best_total {
                // Every remaining state is at least as expensive as the
                // best complete route, and exiting never costs less than
                // nothing, so none of them can improve it.
                break;
            }
            if spent > best_cost[node] {
                continue;
            }
            let gate = &self.gates[node];
            if let Some(exit) = self.exit_cost(gate, &to, &pricing)
                && spent + exit < best_total
            {
                best_total = spent + exit;
                best_node = Some(node);
            }
            // Legs to the other gates of the same system.
            if let Some(neighbours) = self.by_system.get(&gate.system_id) {
                for other in neighbours {
                    if *other == node {
                        continue;
                    }
                    let next = spent + pricing.leg(gate.distance_to(self.gates[*other].position));
                    if next < best_cost[*other] {
                        best_cost[*other] = next;
                        previous[*other] = Some(node);
                        queue.push(Visit {
                            cost: next,
                            node: *other,
                        });
                    }
                }
            }
            // The jump through the gate.
            if let Some(other) = self.index.get(&gate.destination_id) {
                let next = spent + pricing.jump;
                if next < best_cost[*other] {
                    best_cost[*other] = next;
                    previous[*other] = Some(node);
                    queue.push(Visit {
                        cost: next,
                        node: *other,
                    });
                }
            }
        }

        let Some(last) = best_node else {
            return Ok(None);
        };
        let mut path = vec![last];
        let mut current = last;
        while let Some(before) = previous[current] {
            path.push(before);
            current = before;
        }
        path.reverse();
        Ok(Some(self.describe(
            &path,
            &from,
            &to,
            &pricing,
            best_total.primary,
        )))
    }

    /// Fails if the endpoint names a gate or system the graph does not have.
    fn check_endpoint(&self, endpoint: &Endpoint) -> Result<(), Error> {
        match endpoint {
            Endpoint::Gate(id) if !self.index.contains_key(id) => {
                Err(Error::route(format!("unknown stargate {id}")))
            }
            Endpoint::System(id) | Endpoint::Point { system_id: id, .. }
                if !self.by_system.contains_key(id) =>
            {
                Err(Error::route(format!(
                    "unknown solar system {id}, or it has no stargates"
                )))
            }
            _ => Ok(()),
        }
    }

    /// The route between two endpoints of the same system when neither is a
    /// gate: one leg between the two points, or nothing if either one
    /// has no position.
    fn direct_route(from: &Endpoint, to: &Endpoint, pricing: &Pricing) -> Route {
        let distance = match (from, to) {
            (Endpoint::Point { position: a, .. }, Endpoint::Point { position: b, .. }) => {
                let dx = a[0] - b[0];
                let dy = a[1] - b[1];
                let dz = a[2] - b[2];
                Some((dx * dx + dy * dy + dz * dz).sqrt())
            }
            _ => None,
        };
        match distance {
            Some(distance) => Route {
                gates: Vec::new(),
                jumps: 0,
                legs: 1,
                distance,
                cost: pricing.leg(distance).primary,
            },
            None => Route {
                gates: Vec::new(),
                jumps: 0,
                legs: 0,
                distance: 0.0,
                cost: 0.0,
            },
        }
    }

    /// The gates a search can start from, with what it costs to be there.
    fn entry_costs(&self, from: &Endpoint, pricing: &Pricing) -> Vec<(usize, Cost)> {
        match from {
            Endpoint::Gate(id) => vec![(self.index[id], Cost::ZERO)],
            Endpoint::System(id) => self.by_system[id]
                .iter()
                .map(|node| (*node, Cost::ZERO))
                .collect(),
            Endpoint::Point {
                system_id,
                position,
            } => self.by_system[system_id]
                .iter()
                .map(|node| (*node, pricing.leg(self.gates[*node].distance_to(*position))))
                .collect(),
        }
    }

    /// What it costs to finish at `to` when standing at `gate`, or `None`
    /// if `gate` is not a valid way to finish there.
    fn exit_cost(&self, gate: &Gate, to: &Endpoint, pricing: &Pricing) -> Option<Cost> {
        match to {
            Endpoint::Gate(id) => (gate.id == *id).then_some(Cost::ZERO),
            Endpoint::System(id) => (gate.system_id == *id).then_some(Cost::ZERO),
            Endpoint::Point {
                system_id,
                position,
            } => (gate.system_id == *system_id).then(|| pricing.leg(gate.distance_to(*position))),
        }
    }

    /// Turns a path of node indexes into a [`Route`], recomputing the
    /// breakdown from the path itself. `primary` is the value of the
    /// quantity the search minimised first.
    fn describe(
        &self,
        path: &[usize],
        from: &Endpoint,
        to: &Endpoint,
        pricing: &Pricing,
        primary: f64,
    ) -> Route {
        let mut distance = 0.0;
        let mut legs = 0;
        let mut jumps = 0;
        if let (Endpoint::Point { position, .. }, Some(first)) = (from, path.first()) {
            distance += self.gates[*first].distance_to(*position);
            legs += 1;
        }
        for pair in path.windows(2) {
            let (a, b) = (&self.gates[pair[0]], &self.gates[pair[1]]);
            if a.system_id == b.system_id {
                distance += a.distance_to(b.position);
                legs += 1;
            } else {
                jumps += 1;
            }
        }
        if let (Endpoint::Point { position, .. }, Some(last)) = (to, path.last()) {
            distance += self.gates[*last].distance_to(*position);
            legs += 1;
        }
        debug_assert!(
            (distance * pricing.per_meter.primary
                + legs as f64 * pricing.leg_fixed.primary
                + jumps as f64 * pricing.jump.primary
                - primary)
                .abs()
                <= 1e-6 * primary.max(1.0),
            "the breakdown must add up to the cost the search found"
        );
        Route {
            gates: path.iter().map(|node| self.gates[*node].id).collect(),
            jumps,
            legs,
            distance,
            cost: primary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a gate whose destination system is looked up from `id`
    /// (`id / 10` is the system).
    fn gate(id: usize, destination_id: usize, x: f64) -> Gate {
        Gate {
            id,
            system_id: id / 10,
            destination_id,
            destination_system_id: destination_id / 10,
            position: [x, 0.0, 0.0],
        }
    }

    /// ```text
    /// system 1: gate 11 (x=0)   -> 21        gate 13 (x=500) -> 43
    /// system 2: gate 21 (x=0)   -> 11        gate 22 (x=50)  -> 41
    /// system 4: gate 41 (x=0)   -> 22        gate 43 (x=0)   -> 13
    /// system 5: gate 51 (x=0)   -> 99 (not in the graph)
    /// ```
    ///
    /// From system 1 to system 4 there are two routes: two jumps through
    /// system 2 with very little flying, or one jump with a long flight to
    /// gate 13.
    fn sample() -> GateGraph {
        GateGraph::from_gates(vec![
            gate(11, 21, 0.0),
            gate(13, 43, 500.0),
            gate(21, 11, 0.0),
            gate(22, 41, 50.0),
            gate(41, 22, 0.0),
            gate(43, 13, 0.0),
            gate(51, 99, 0.0),
        ])
    }

    fn start() -> Endpoint {
        Endpoint::Point {
            system_id: 1,
            position: [0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn counts_and_lookups() {
        let graph = sample();
        assert_eq!(graph.gate_count(), 7);
        assert_eq!(graph.system_count(), 4);
        assert_eq!(graph.gate(22).unwrap().system_id, 2);
        assert!(graph.gate(99).is_none());
        assert_eq!(graph.gates_in_system(1).count(), 2);
        assert_eq!(graph.gates_in_system(7).count(), 0);
    }

    #[test]
    fn duplicate_gate_ids_keep_the_first() {
        let graph = GateGraph::from_gates(vec![gate(11, 21, 0.0), gate(11, 21, 999.0)]);
        assert_eq!(graph.gate_count(), 1);
        assert_eq!(graph.gate(11).unwrap().position[0], 0.0);
    }

    #[test]
    fn cheap_jumps_prefer_the_short_flight() {
        let route = sample()
            .route(
                start(),
                Endpoint::System(4),
                &Priority::balanced(100.0, 0.0),
            )
            .unwrap()
            .unwrap();
        // 0 (to gate 11) + 100 + 50 (21 -> 22) + 100
        assert_eq!(route.gates, vec![11, 21, 22, 41]);
        assert_eq!(route.jumps, 2);
        assert_eq!(route.legs, 2);
        assert_eq!(route.distance, 50.0);
        assert_eq!(route.cost, 250.0);
    }

    #[test]
    fn expensive_jumps_prefer_fewer_of_them() {
        let route = sample()
            .route(
                start(),
                Endpoint::System(4),
                &Priority::balanced(1000.0, 0.0),
            )
            .unwrap()
            .unwrap();
        // 500 (to gate 13) + 1000
        assert_eq!(route.gates, vec![13, 43]);
        assert_eq!(route.jumps, 1);
        assert_eq!(route.legs, 1);
        assert_eq!(route.distance, 500.0);
        assert_eq!(route.cost, 1500.0);
    }

    #[test]
    fn a_cost_per_leg_penalises_many_legs() {
        // With jump_cost = 100 the route through system 2 wins (250), but
        // each leg costing 1000 makes it much worse than the single leg
        // to gate 13.
        let route = sample()
            .route(
                start(),
                Endpoint::System(4),
                &Priority::balanced(100.0, 1000.0),
            )
            .unwrap()
            .unwrap();
        assert_eq!(route.gates, vec![13, 43]);
        assert_eq!(route.cost, 500.0 + 100.0 + 1000.0);
    }

    #[test]
    fn breakdown_adds_up() {
        let cost = RouteCost::new(70.0, 3.0);
        let priority = Priority::Balanced(cost);
        let route = sample()
            .route(start(), Endpoint::Gate(41), &priority)
            .unwrap()
            .unwrap();
        let rebuilt = route.distance
            + route.legs as f64 * cost.leg_cost
            + route.jumps as f64 * cost.jump_cost;
        assert!((rebuilt - route.cost).abs() < 1e-9);
    }

    #[test]
    fn starting_at_a_gate_costs_nothing_to_reach_it() {
        let route = sample()
            .route(
                Endpoint::Gate(11),
                Endpoint::Gate(21),
                &Priority::balanced(5.0, 9.0),
            )
            .unwrap()
            .unwrap();
        assert_eq!(route.gates, vec![11, 21]);
        assert_eq!(route.legs, 0);
        assert_eq!(route.cost, 5.0);
    }

    #[test]
    fn same_gate_is_a_route_of_one_gate() {
        let route = sample()
            .route(Endpoint::Gate(22), Endpoint::Gate(22), &Priority::default())
            .unwrap()
            .unwrap();
        assert_eq!(route.gates, vec![22]);
        assert_eq!(route.cost, 0.0);
    }

    #[test]
    fn same_system_points_are_one_leg() {
        let route = sample()
            .route(
                Endpoint::Point {
                    system_id: 1,
                    position: [0.0, 0.0, 0.0],
                },
                Endpoint::Point {
                    system_id: 1,
                    position: [3.0, 4.0, 0.0],
                },
                &Priority::balanced(1.0, 2.0),
            )
            .unwrap()
            .unwrap();
        assert!(route.gates.is_empty());
        assert_eq!(route.legs, 1);
        assert_eq!(route.distance, 5.0);
        assert_eq!(route.cost, 7.0);
    }

    #[test]
    fn same_system_without_positions_costs_nothing() {
        let route = sample()
            .route(
                Endpoint::System(1),
                Endpoint::System(1),
                &Priority::default(),
            )
            .unwrap()
            .unwrap();
        assert!(route.gates.is_empty());
        assert_eq!(route.cost, 0.0);
    }

    #[test]
    fn unreachable_endpoints_give_none() {
        let route = sample()
            .route(
                Endpoint::System(1),
                Endpoint::System(5),
                &Priority::default(),
            )
            .unwrap();
        assert!(route.is_none());
    }

    #[test]
    fn unknown_endpoints_are_errors() {
        let graph = sample();
        let cost = Priority::default();
        assert!(
            graph
                .route(Endpoint::Gate(12345), Endpoint::System(1), &cost)
                .is_err()
        );
        assert!(
            graph
                .route(Endpoint::System(1), Endpoint::System(777), &cost)
                .is_err()
        );
        assert!(
            graph
                .route(
                    Endpoint::System(1),
                    Endpoint::Point {
                        system_id: 777,
                        position: [0.0; 3]
                    },
                    &cost
                )
                .is_err()
        );
    }

    #[test]
    fn invalid_costs_are_errors() {
        let graph = sample();
        for cost in [
            Priority::balanced(-1.0, 0.0),
            Priority::balanced(1.0, -0.5),
            Priority::balanced(f64::NAN, 0.0),
            Priority::balanced(1.0, f64::INFINITY),
        ] {
            assert!(
                graph
                    .route(Endpoint::System(1), Endpoint::System(4), &cost)
                    .is_err()
            );
        }
    }

    #[test]
    fn crossing_a_system_costs_the_distance_between_its_gates() {
        // This is the reason the nodes are gates: entering system 2
        // through gate 21 and leaving through gate 22 costs the 50
        // between them even with free jumps.
        let route = sample()
            .route(
                Endpoint::Gate(11),
                Endpoint::Gate(41),
                &Priority::balanced(0.0, 0.0),
            )
            .unwrap()
            .unwrap();
        assert_eq!(route.gates, vec![11, 21, 22, 41]);
        assert_eq!(route.distance, 50.0);
    }

    #[test]
    fn fewest_jumps_ignores_the_distance() {
        let route = sample()
            .route(start(), Endpoint::System(4), &Priority::FewestJumps)
            .unwrap()
            .unwrap();
        assert_eq!(route.gates, vec![13, 43]);
        assert_eq!(route.jumps, 1);
        assert_eq!(route.cost, 1.0);
    }

    #[test]
    fn shortest_flight_ignores_the_jumps() {
        let route = sample()
            .route(start(), Endpoint::System(4), &Priority::ShortestFlight)
            .unwrap()
            .unwrap();
        assert_eq!(route.gates, vec![11, 21, 22, 41]);
        assert_eq!(route.jumps, 2);
        assert_eq!(route.distance, 50.0);
        assert_eq!(route.cost, 50.0);
    }

    /// Two routes with the same number of jumps and different distances:
    /// through gate 14 (100 away from the start) or through gate 13
    /// (500 away).
    fn two_single_jump_routes() -> GateGraph {
        GateGraph::from_gates(vec![
            gate(13, 43, 500.0),
            gate(14, 44, 100.0),
            gate(43, 13, 0.0),
            gate(44, 14, 0.0),
        ])
    }

    #[test]
    fn fewest_jumps_breaks_ties_with_the_shortest_flight() {
        let route = two_single_jump_routes()
            .route(
                Endpoint::Point {
                    system_id: 1,
                    position: [0.0; 3],
                },
                Endpoint::System(4),
                &Priority::FewestJumps,
            )
            .unwrap()
            .unwrap();
        assert_eq!(route.gates, vec![14, 44]);
        assert_eq!(route.distance, 100.0);
    }

    /// Two routes with the same flight distance (zero) and a different
    /// number of jumps: one jump through gate 15, or two through system 2.
    fn same_distance_different_jumps() -> GateGraph {
        GateGraph::from_gates(vec![
            gate(11, 21, 0.0),
            gate(15, 45, 0.0),
            gate(21, 11, 0.0),
            gate(22, 41, 0.0),
            gate(41, 22, 0.0),
            gate(45, 15, 0.0),
        ])
    }

    #[test]
    fn shortest_flight_breaks_ties_with_the_fewest_jumps() {
        let route = same_distance_different_jumps()
            .route(
                Endpoint::System(1),
                Endpoint::System(4),
                &Priority::ShortestFlight,
            )
            .unwrap()
            .unwrap();
        assert_eq!(route.gates, vec![15, 45]);
        assert_eq!(route.jumps, 1);
        assert_eq!(route.distance, 0.0);
    }

    #[test]
    fn the_default_priority_is_balanced() {
        assert_eq!(
            Priority::default(),
            Priority::Balanced(RouteCost::default())
        );
    }
}
