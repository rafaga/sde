use super::SdePoint;

/// Note: no longer derives `Hash`/`Eq` (only `PartialEq`) since `min`/`max`
/// became `SdePoint`, which contains `[f64; 3]` -- `f64` doesn't
/// implement `Eq`/`Hash` (NaN isn't equal to itself). Nothing in this
/// crate used `EveRegionArea` as a `HashMap`/`HashSet` key or otherwise
/// relied on those two traits (confirmed: it's only ever returned inside
/// a `Vec`).
#[derive(PartialEq, Clone, Debug)]
pub struct EveRegionArea {
    pub region_id: i64,
    pub name: String,
    pub min: SdePoint,
    pub max: SdePoint,
}

impl Default for EveRegionArea {
    fn default() -> Self {
        Self::new()
    }
}

impl EveRegionArea {
    pub fn new() -> Self {
        EveRegionArea {
            region_id: 0,
            name: String::new(),
            min: SdePoint::default(),
            max: SdePoint::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everegionarea_new_is_empty() {
        let area = EveRegionArea::new();
        assert_eq!(area.region_id, 0);
        assert_eq!(area.name, String::new());
        assert_eq!(area.min, SdePoint::default());
        assert_eq!(area.max, SdePoint::default());
        assert_eq!(area, EveRegionArea::default());
    }
}
