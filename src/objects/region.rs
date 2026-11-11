use super::SdePoint;

/// Abstraction for a Region. It store data relevant to this entity
///
/// Note: no longer derives `Hash`/`Eq`, same reason and same
/// confirmation (`HashMap` value, never a key) as [`SolarSystem`](super::SolarSystem).
#[derive(PartialEq, Clone, Debug)]
pub struct Region {
    /// Region Identifier
    pub id: u32,
    /// Region Name
    pub name: String,
    /// Vector with Region's Constellationm Identifiers
    pub constellations: Vec<u32>,
    /// Wormhole class of the region (`mapRegions.wormholeClassId`): 1 to 6 for
    /// the wormhole regions ("C1" to "C6"), 12 for Thera, 13 for the shattered
    /// ones, 19 to 23 for the abyssal ones, 25 for Pochven. The regions of
    /// k-space carry 7, 8 or 9 (high, low and null security). `None` if it has
    /// none. The systems of the region share it: see
    /// [`Universe::wormhole_class`](super::Universe::wormhole_class).
    pub wormhole_class: Option<u32>,
    /// Region 2D Coordinates with the propourse of representing the system in abstraction map.
    pub projected_coords: SdePoint,
}

impl Region {
    /// Creates a new Region Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Region {
            id: 0,
            name: String::new(),
            constellations: Vec::new(),
            wormhole_class: None,
            projected_coords: SdePoint::default(),
        }
    }
}

impl Default for Region {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_new_is_empty() {
        let region = Region::new();
        assert_eq!(region.id, 0);
        assert_eq!(region.name, String::new());
        assert!(region.constellations.is_empty());
        assert_eq!(region.projected_coords, SdePoint::default());
        assert_eq!(region, Region::default());
    }
}
