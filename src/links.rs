//! Connections between systems that are not stargates: the wormholes that
//! lead to Thera or Turnur today, and the Ansiblex bridges later.
//!
//! The [`Universe`] says nothing about them and does not change: Thera is a
//! single system with its real id and no gates, like every system of
//! wormhole space. What is known about the extra connections lives in a
//! [`LinkLayer`] next to it, and whoever plans a route or draws a map asks
//! the layer, not the universe, for the neighbours of a system that include
//! them ([`LinkLayer::neighbours`]).
//!
//! The layer has no clock: wormholes collapse, so everything that depends on
//! the time takes the moment to look at (`now`) from the caller. That keeps
//! it deterministic and testable.
//!
//! The layer does not depend on where its links come from. EvE-Scout's
//! signatures, for one, are turned into [`Link`]s by the application.

use crate::objects::Universe;
use chrono::{DateTime, Utc};

/// A wormhole between two systems, as the two signatures that show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wormhole {
    /// The signature in the origin system (`ABC-123`).
    pub out_signature: String,
    /// The signature in the destination system, once it is known.
    pub in_signature: Option<String>,
    /// When it is expected to collapse, if it is known.
    pub expires_at: Option<DateTime<Utc>>,
}

/// What a [`Link`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinkSource {
    /// A wormhole, which anybody can jump through until it collapses.
    Wormhole(Wormhole),
}

/// A connection between two systems that is not a stargate.
///
/// It can be used in both directions: that is how a wormhole works, and the
/// origin only says where the signature that found it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The system where the connection was found (`SolarSystem::id`).
    pub origin: u32,
    /// The system on the other side.
    pub destination: u32,
    /// What kind of connection it is.
    pub source: LinkSource,
}

impl Link {
    /// A wormhole from `origin` to `destination`.
    pub fn wormhole(origin: u32, destination: u32, wormhole: Wormhole) -> Self {
        Self {
            origin,
            destination,
            source: LinkSource::Wormhole(wormhole),
        }
    }

    /// Whether the connection can still be used at `now`. A wormhole whose
    /// collapse is not known is taken as usable.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        match &self.source {
            LinkSource::Wormhole(wormhole) => wormhole
                .expires_at
                .is_none_or(|expires_at| expires_at > now),
        }
    }

    /// The system on the other side of the connection from `system`, or
    /// `None` if the link does not touch it.
    pub fn other_end(&self, system: u32) -> Option<u32> {
        if self.origin == system {
            Some(self.destination)
        } else if self.destination == system {
            Some(self.origin)
        } else {
            None
        }
    }
}

/// The extra connections between systems that are known at some moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkLayer {
    links: Vec<Link>,
}

impl LinkLayer {
    /// A layer without links.
    pub fn new() -> Self {
        Self::default()
    }

    /// A layer with `links`.
    pub fn from_links(links: Vec<Link>) -> Self {
        Self { links }
    }

    /// Adds a link.
    pub fn add(&mut self, link: Link) {
        self.links.push(link);
    }

    /// Takes every link out of the layer.
    pub fn clear(&mut self) {
        self.links.clear();
    }

    /// Replaces every link, for a source that is read again as a whole.
    pub fn replace(&mut self, links: Vec<Link>) {
        self.links = links;
    }

    /// How many links the layer has, whether or not they are active.
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// Whether the layer has no links.
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// Every link, active or not.
    pub fn links(&self) -> &[Link] {
        &self.links
    }

    /// The links that can be used at `now`.
    pub fn active(&self, now: DateTime<Utc>) -> impl Iterator<Item = &Link> {
        self.links.iter().filter(move |link| link.is_active(now))
    }

    /// The systems `system` is connected to at `now`: the ones its stargates
    /// lead to, as the `universe` has them, followed by the ones an active
    /// link leads to. A system comes out once, and a system that is not in
    /// the universe has only the links.
    ///
    /// The universe is not changed, so whoever must keep seeing gates only
    /// (the intel alarms, the kill counts) goes on reading
    /// `SolarSystem::connections`.
    pub fn neighbours(&self, universe: &Universe, system: u32, now: DateTime<Utc>) -> Vec<u32> {
        let mut neighbours: Vec<u32> = universe
            .solar_systems
            .get(&system)
            .map(|solar_system| solar_system.connections.clone())
            .unwrap_or_default();
        for other in self.active(now).filter_map(|link| link.other_end(system)) {
            if !neighbours.contains(&other) {
                neighbours.push(other);
            }
        }
        neighbours
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::SolarSystem;
    use chrono::TimeDelta;

    const THERA: u32 = 31_000_005;
    const JITA: u32 = 30_000_142;
    const NEW_CAULDRON: u32 = 30_000_139;
    const PERIMETER: u32 = 30_000_144;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn wormhole(expires_in: Option<TimeDelta>) -> Wormhole {
        Wormhole {
            out_signature: String::from("ABC-123"),
            in_signature: Some(String::from("XYZ-789")),
            expires_at: expires_in.map(|left| now() + left),
        }
    }

    fn universe() -> Universe {
        let mut universe = Universe::new(1.0);
        for (id, connections) in [
            (JITA, vec![NEW_CAULDRON, PERIMETER]),
            (NEW_CAULDRON, vec![JITA]),
            (PERIMETER, vec![JITA]),
            (THERA, Vec::new()),
        ] {
            let system = SolarSystem {
                id,
                connections,
                ..SolarSystem::default()
            };
            universe.solar_systems.insert(id, system);
        }
        universe
    }

    #[test]
    fn a_wormhole_can_be_used_until_it_collapses() {
        let link = Link::wormhole(JITA, THERA, wormhole(Some(TimeDelta::hours(2))));
        assert!(link.is_active(now()));
        assert!(link.is_active(now() + TimeDelta::hours(1)));
        assert!(!link.is_active(now() + TimeDelta::hours(2)));
        assert!(!link.is_active(now() + TimeDelta::hours(3)));
    }

    #[test]
    fn a_wormhole_with_no_known_collapse_is_taken_as_usable() {
        let link = Link::wormhole(JITA, THERA, wormhole(None));
        assert!(link.is_active(now() + TimeDelta::days(30)));
    }

    #[test]
    fn a_link_is_followed_from_either_end() {
        let link = Link::wormhole(JITA, THERA, wormhole(None));
        assert_eq!(link.other_end(JITA), Some(THERA));
        assert_eq!(link.other_end(THERA), Some(JITA));
        assert_eq!(link.other_end(PERIMETER), None);
    }

    #[test]
    fn the_neighbours_are_the_gates_and_then_the_active_links() {
        let universe = universe();
        let layer = LinkLayer::from_links(vec![Link::wormhole(
            THERA,
            JITA,
            wormhole(Some(TimeDelta::hours(5))),
        )]);
        assert_eq!(
            layer.neighbours(&universe, JITA, now()),
            vec![NEW_CAULDRON, PERIMETER, THERA]
        );
        // Thera has no gates: the link is its only way out.
        assert_eq!(layer.neighbours(&universe, THERA, now()), vec![JITA]);
    }

    #[test]
    fn a_collapsed_wormhole_is_not_a_neighbour() {
        let universe = universe();
        let layer = LinkLayer::from_links(vec![Link::wormhole(
            THERA,
            JITA,
            wormhole(Some(TimeDelta::hours(1))),
        )]);
        let later = now() + TimeDelta::hours(2);
        assert_eq!(
            layer.neighbours(&universe, JITA, later),
            vec![NEW_CAULDRON, PERIMETER]
        );
        assert!(layer.neighbours(&universe, THERA, later).is_empty());
    }

    #[test]
    fn a_system_comes_out_once_even_if_a_link_repeats_a_gate() {
        let universe = universe();
        let layer = LinkLayer::from_links(vec![
            Link::wormhole(JITA, PERIMETER, wormhole(None)),
            Link::wormhole(THERA, JITA, wormhole(None)),
            Link::wormhole(JITA, THERA, wormhole(None)),
        ]);
        assert_eq!(
            layer.neighbours(&universe, JITA, now()),
            vec![NEW_CAULDRON, PERIMETER, THERA]
        );
    }

    #[test]
    fn a_system_that_is_not_in_the_universe_has_only_the_links() {
        let layer = LinkLayer::from_links(vec![Link::wormhole(THERA, 31_001_000, wormhole(None))]);
        assert_eq!(
            layer.neighbours(&Universe::new(1.0), 31_001_000, now()),
            vec![THERA]
        );
    }

    #[test]
    fn the_universe_is_left_as_it_was() {
        let universe = universe();
        let layer = LinkLayer::from_links(vec![Link::wormhole(THERA, JITA, wormhole(None))]);
        let _ = layer.neighbours(&universe, JITA, now());
        assert!(universe.solar_systems[&THERA].connections.is_empty());
        assert_eq!(
            universe.solar_systems[&JITA].connections,
            vec![NEW_CAULDRON, PERIMETER]
        );
    }

    #[test]
    fn the_layer_is_replaced_as_a_whole() {
        let mut layer = LinkLayer::new();
        assert!(layer.is_empty());
        layer.add(Link::wormhole(THERA, JITA, wormhole(None)));
        assert_eq!(layer.len(), 1);
        layer.replace(vec![
            Link::wormhole(THERA, PERIMETER, wormhole(None)),
            Link::wormhole(THERA, NEW_CAULDRON, wormhole(None)),
        ]);
        assert_eq!(layer.len(), 2);
        assert_eq!(layer.links()[0].destination, PERIMETER);
        layer.clear();
        assert!(layer.is_empty());
    }

    #[test]
    fn only_the_active_links_are_listed_as_active() {
        let layer = LinkLayer::from_links(vec![
            Link::wormhole(THERA, JITA, wormhole(Some(TimeDelta::hours(1)))),
            Link::wormhole(THERA, PERIMETER, wormhole(Some(TimeDelta::hours(5)))),
        ]);
        let later = now() + TimeDelta::hours(2);
        let active: Vec<_> = layer.active(later).map(|link| link.destination).collect();
        assert_eq!(active, vec![PERIMETER]);
    }
}
