use super::Position2DMode;
use sha2::{Digest, Sha256};

/// A record of the settings used to build a specific `sde.db`, plus a
/// SHA-256 hash over them and the SDE build number -- lets a consumer
/// detect whether the database (or at least this one table) was
/// hand-edited after `sde-builder` generated it.
///
/// This is tamper-**evidence**, not tamper-**proof**: the hash has no
/// secret key, and `sde-builder` is a tool anyone can run themselves
/// against their own database, so there's no party who holds a private
/// key the way a code-signing certificate would. Anyone with access to
/// this crate's source (public, on crates.io/GitHub) can recompute a
/// valid hash for any values they want to write. It answers "does this
/// table's content match what `sde-builder` actually produced", not
/// "can I trust this database came from a specific, authorized build".
/// If that stronger guarantee is ever needed, it requires a genuinely
/// different mechanism (asymmetric signing, with the private key held
/// only by a trusted build pipeline that end users never run
/// themselves) -- not a variation on this one.
#[derive(Debug, Clone, PartialEq)]
pub struct SdeFingerprint {
    /// SDE build number (from `latest.jsonl`/the local `.build` file),
    /// if the caller that built this database had one available.
    pub sde_build: Option<String>,
    pub language: String,
    /// How `position2DX`/`position2DY` were produced (see
    /// [`Position2DMode`]). Replaces the old
    /// `force_isometric_position_2d: bool` +
    /// `isometric_projected_axis: ProjectedAxis` pair -- note this
    /// changes [`Self::to_hash_input`]'s format, so fingerprints
    /// written by older versions of this crate no longer verify
    /// (acceptable: the table exists to detect config mismatches, and
    /// any config change means a rebuild anyway).
    pub position_2d: Position2DMode,
    pub map_kspace: bool,
    pub map_wspace: bool,
    pub map_abyssal: bool,
    pub map_void: bool,
    pub with_gates: bool,
    pub with_moons: bool,
    pub with_third_party: bool,
    /// `None` when `with_third_party` is `false` (the four
    /// `CommunityConfig` flags below it are never consulted in that
    /// case, so there's nothing meaningful to record).
    pub with_icebelts: Option<bool>,
    pub with_triglavian_status: Option<bool>,
    pub with_jove_observatories: Option<bool>,
    pub with_special_ore: Option<bool>,
}

impl SdeFingerprint {
    /// Canonical string these fields hash to. Used both when writing the
    /// fingerprint (`builder::parser::Parser::build_database`) and when
    /// verifying it (`SdeManager::get_fingerprint`) -- kept in this one
    /// place, in a crate location neither side is gated away from, so
    /// the two can never drift out of sync with each other.
    ///
    /// Deliberately a plain `format!` with `|` separators instead of a
    /// general-purpose serialization format (JSON, etc.): a
    /// serialization library's exact byte output isn't part of its
    /// documented contract and can change between versions without it
    /// being considered a breaking change, which would silently change
    /// every previously-computed hash's meaning. This has no such
    /// external dependency, so it can only change here, deliberately.
    fn to_hash_input(&self) -> String {
        format!(
            "{}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}",
            self.sde_build.as_deref().unwrap_or(""),
            self.language,
            self.position_2d,
            self.map_kspace,
            self.map_wspace,
            self.map_abyssal,
            self.map_void,
            self.with_gates,
            self.with_moons,
            self.with_third_party,
            self.with_icebelts,
            self.with_triglavian_status,
            self.with_jove_observatories,
            self.with_special_ore,
        )
    }

    /// SHA-256 hex digest of [`Self::to_hash_input`].
    pub fn hash(&self) -> String {
        let digest = Sha256::digest(self.to_hash_input().as_bytes());
        digest
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect::<String>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fingerprint() -> SdeFingerprint {
        SdeFingerprint {
            sde_build: Some("3458726".to_string()),
            language: "en".to_string(),
            position_2d: Position2DMode::Ccp,
            map_kspace: true,
            map_wspace: true,
            map_abyssal: true,
            map_void: false,
            with_gates: true,
            with_moons: true,
            with_third_party: false,
            with_icebelts: None,
            with_triglavian_status: None,
            with_jove_observatories: None,
            with_special_ore: None,
        }
    }

    #[test]
    fn fingerprint_hash_is_deterministic() {
        assert_eq!(sample_fingerprint().hash(), sample_fingerprint().hash());
    }

    #[test]
    fn fingerprint_hash_is_a_64_char_hex_string() {
        let hash = sample_fingerprint().hash();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_hash_changes_when_a_single_flag_changes() {
        let base = sample_fingerprint();
        let mut changed = sample_fingerprint();
        changed.with_moons = false;
        assert_ne!(base.hash(), changed.hash());
    }

    #[test]
    fn fingerprint_hash_changes_when_sde_build_changes() {
        let base = sample_fingerprint();
        let mut changed = sample_fingerprint();
        changed.sde_build = Some("9999999".to_string());
        assert_ne!(base.hash(), changed.hash());
    }

    #[test]
    fn fingerprint_hash_distinguishes_none_from_explicit_values() {
        // A subtle case worth confirming directly: with_third_party's
        // four dependent flags at None (not consulted) must hash
        // differently than if they had been explicitly Some(false) --
        // otherwise "third-party data was never attempted" and
        // "third-party data was attempted with everything disabled"
        // would be indistinguishable.
        let mut none_case = sample_fingerprint();
        none_case.with_third_party = true;
        let mut false_case = sample_fingerprint();
        false_case.with_third_party = true;
        false_case.with_icebelts = Some(false);
        false_case.with_triglavian_status = Some(false);
        false_case.with_jove_observatories = Some(false);
        false_case.with_special_ore = Some(false);
        assert_ne!(none_case.hash(), false_case.hash());
    }
}
