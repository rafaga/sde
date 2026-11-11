use super::{SdePoint, Star};

/// Abstraction for a Solar System. It store data relevant to this entity
///
/// Note: no longer derives `Hash`/`Eq` (only `PartialEq`) -- same reason
/// as [`EveRegionArea`](super::EveRegionArea): `real_coords`/`projected_coords` became
/// `SdePoint`, which contains `[f64; 3]`. Confirmed safe: `SolarSystem`
/// is only ever a `HashMap` *value* (`HashMap<u32, SolarSystem>`), never
/// a key.
#[derive(PartialEq, Clone, Debug)]
pub struct SolarSystem {
    /// Solar System identifier
    pub id: u32,
    /// Solar System name
    pub name: String,
    /// Region identifier
    pub region: u32,
    /// Constellation identifier
    pub constellation: u32,
    /// True security status (`mapSolarSystems.security`, -1.0 to 1.0), as
    /// read by [`SdeManager::get_universe`](crate::SdeManager::get_universe);
    /// `0.0` in a system that wasn't read from the database.
    pub security: f64,
    /// Wormhole class of the system itself (`mapSolarSystems.wormholeClassId`),
    /// which only the drifter systems (14 to 18) have: the others take the one
    /// of their region. See
    /// [`Universe::wormhole_class`](super::Universe::wormhole_class), which
    /// gives the one that applies.
    pub wormhole_class: Option<u32>,
    /// Planet vector with Identifer numbers in their respective cardinal order
    pub planets: Vec<u32>,
    /// Vector with Solar system identifiers where this Solar system has connections via Stargates
    pub connections: Vec<u32>,
    /// Solar System 3D Coordinates
    pub real_coords: SdePoint,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: SdePoint,
    /// Item categories entirely disallowed from being anchored here
    /// (e.g. Structure, Deployable) -- via
    /// `mapSolarSystemDisallowedAnchorableCategories`. Confirmed
    /// independent of `disallowed_anchor_groups` against real data (a
    /// system can restrict one without the other) -- see
    /// [ERD.md](https://github.com/rafaga/sde/blob/main/ERD.md).
    pub disallowed_anchor_categories: Vec<u32>,
    /// Specific item groups disallowed from being anchored here (e.g.
    /// Mobile Warp Disruptor), independent of the category-level
    /// restriction above -- via
    /// `mapSolarSystemDisallowedAnchorableGroups`.
    pub disallowed_anchor_groups: Vec<u32>,
    /// This system's star, if it has one -- `None` for the systems
    /// that don't (confirmed against real data, August 2026: 401 of
    /// 8490 real solar systems, 4.7%, have no `mapStars` row at all).
    pub star: Option<Star>,
    /// The factor that we need to adjust the coordinates
    pub factor: f64,
}

impl SolarSystem {
    /// Creates a new Solar System Strcut. ALl the values are initialized. Needs to be filled
    pub fn new(factor: f64) -> Self {
        SolarSystem {
            id: 0,
            name: String::new(),
            region: 0,
            constellation: 0,
            security: 0.0,
            wormhole_class: None,
            planets: Vec::new(),
            connections: Vec::new(),
            real_coords: SdePoint::default(),
            projected_coords: SdePoint::default(),
            disallowed_anchor_categories: Vec::new(),
            disallowed_anchor_groups: Vec::new(),
            star: None,
            factor,
        }
    }
}

impl Default for SolarSystem {
    fn default() -> Self {
        Self::new(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solarsystem_new_initializes_with_factor() {
        let system = SolarSystem::new(1000.0);
        assert_eq!(system.id, 0);
        assert_eq!(system.name, String::new());
        assert_eq!(system.region, 0);
        assert_eq!(system.constellation, 0);
        assert!(system.planets.is_empty());
        assert!(system.connections.is_empty());
        assert_eq!(system.real_coords, SdePoint::default());
        assert_eq!(system.projected_coords, SdePoint::default());
        assert_eq!(system.factor, 1000.0);
    }

    #[test]
    fn solarsystem_default_factor_is_one() {
        assert_eq!(SolarSystem::default().factor, 1.0);
    }
}
