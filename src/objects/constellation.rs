use super::SdePoint;

/// Abstraction for a Constellation. It store data relevant to this entity
///
/// Note: no longer derives `Hash`/`Eq`, same reason and same
/// confirmation (`HashMap` value, never a key) as [`SolarSystem`](super::SolarSystem).
#[derive(PartialEq, Clone, Debug)]
pub struct Constellation {
    /// Constellation Identifier
    pub id: u32,
    /// Constellation Name
    pub name: String,
    /// Region Identifier
    pub region: u32,
    /// Solar System vector with Identifer numbers included in the constellation
    pub solar_systems: Vec<u32>,
    /// Solar System 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: SdePoint,
}

impl Constellation {
    /// Creates a new Constellation Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Constellation {
            id: 0,
            name: String::new(),
            region: 0,
            solar_systems: Vec::new(),
            projected_coords: SdePoint::default(),
        }
    }
}

impl Default for Constellation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constellation_new_is_empty() {
        let constellation = Constellation::new();
        assert_eq!(constellation.id, 0);
        assert_eq!(constellation.name, String::new());
        assert_eq!(constellation.region, 0);
        assert!(constellation.solar_systems.is_empty());
        assert_eq!(constellation.projected_coords, SdePoint::default());
        assert_eq!(constellation, Constellation::default());
    }
}
