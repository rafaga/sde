/// Abstraction for a Moon. It store data relevant to this entity
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub struct Moon {
    /// Moon Identifier
    pub id: u32,
    /// Moon's Planet identifier
    pub planet: u32,
    /// The cardinal number of this moon in the planet
    pub index: u8,
    /// Moon's Solar System Identifier
    pub solar_system: u32,
}

impl Moon {
    /// Creates a new Moon Strcut. ALl the values are initialized. Needs to be filled
    pub fn new() -> Self {
        Moon {
            id: 0,
            planet: 0,
            index: 0,
            solar_system: 0,
        }
    }
}

impl Default for Moon {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moon_new_is_zeroed() {
        let moon = Moon::new();
        assert_eq!(moon.id, 0);
        assert_eq!(moon.planet, 0);
        assert_eq!(moon.index, 0);
        assert_eq!(moon.solar_system, 0);
        assert_eq!(moon, Moon::default());
    }
}
