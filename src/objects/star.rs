/// The star at the center of a solar system: its own data (from
/// `mapStars`) plus the spectral-class properties shared by every star
/// of its type (from `typeStar`, joined via `mapStars.starTypeId ->
/// typeStar.typeId`).
#[derive(PartialEq, Clone, Debug)]
pub struct Star {
    /// Star identifier (`mapStars.starId`)
    pub id: u32,
    /// Whether the star is tidally locked to its primary in a binary
    /// system. `None` in practice for every real record checked
    /// (August 2026): `mapStars.locked` never actually shows up in the
    /// source data, so this column always reads back `NULL` -- kept as
    /// `Option` rather than defaulting to `false` so "unlocked" and
    /// "not recorded" stay distinguishable.
    pub locked: Option<bool>,
    /// Star radius in meters, if present in the source data.
    pub radius: Option<u32>,
    /// Spectral class (e.g. `"G5"`, `"K7"`) -- `typeStar.name`.
    pub spectral_class: String,
    /// RGB hex color for this spectral class (e.g. `"#FFE996"`), looked
    /// up from the embedded `star_colors.json` at build time --
    /// `typeStar.color`. Shared by every star of the same spectral
    /// class, not unique per star.
    pub color: String,
}
