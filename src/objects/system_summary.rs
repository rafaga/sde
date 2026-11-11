/// What a label or a tooltip needs of a system: its name, its security status,
/// the region it is in and its wormhole class. It is read apart from the rest
/// of the [`Universe`](super::Universe) (planets and moons are the bulk of it).
#[derive(PartialEq, Clone, Debug)]
pub struct SystemSummary {
    /// System name
    pub name: String,
    /// Identifier of the region of the system
    pub region_id: u32,
    /// Name of the region of the system
    pub region_name: String,
    /// True security status of the system (-1.0 to 1.0)
    pub security: f64,
    /// Wormhole class that applies to the system: its own if it has one or the
    /// one of its region, as [`Universe::wormhole_class`](super::Universe::wormhole_class)
    /// gives it.
    pub wormhole_class: Option<u32>,
}
