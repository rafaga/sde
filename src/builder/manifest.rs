//! Manifiesto de fingerprints HTTP para los mapas de dotlan.
//!
//! Equivalente a `_load_manifest` / `_save_manifest` / `_remote_map_fingerprint`
//! del `external_parser.py` original: guarda ETag/Last-Modified/Content-Length
//! por región para no re-descargar un SVG que no cambió en el servidor.
//!
//! Este módulo es deliberadamente puro en cuanto a red: solo sabe leer/escribir
//! el manifiesto en disco y decidir si algo cambió, dado un fingerprint remoto
//! ya obtenido por quien llame. Eso lo hace fácil de testear sin mockear HTTP.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Huella HTTP de un mapa de dotlan en un momento dado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapFingerprint {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<String>,
}

/// Manifiesto completo: nombre de región -> última huella conocida.
pub type Manifest = HashMap<String, MapFingerprint>;

/// Ruta por convención del manifiesto dentro de `<sde_dir>/maps/_manifest.json`.
///
/// Vive deliberadamente dentro de `maps/`, la misma carpeta que se preserva
/// al reconstruir `sde/` (ver `builder::extract`, aún por portar), para que
/// no se pierda entre corridas.
pub fn manifest_path(maps_dir: &Path) -> PathBuf {
    maps_dir.join("_manifest.json")
}

/// Carga el manifiesto desde disco. Si no existe o está corrupto, regresa
/// un manifiesto vacío -- comportamiento seguro: la siguiente corrida
/// simplemente vuelve a descargar todo, nunca se detiene por esto.
pub fn load(maps_dir: &Path) -> Manifest {
    let path = manifest_path(maps_dir);
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|err| {
            eprintln!("http: manifest is not readable in {path:?} ({err}), rebuilding from scratch");
            Manifest::new()
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Manifest::new(),
        Err(err) => {
            eprintln!("http: can't read the manifest file in {path:?} ({err})");
            Manifest::new()
        }
    }
}

/// Guarda el manifiesto en disco (crea `maps_dir` si hace falta).
pub fn save(maps_dir: &Path, manifest: &Manifest) -> io::Result<()> {
    std::fs::create_dir_all(maps_dir)?;
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    std::fs::write(manifest_path(maps_dir), json)
}

/// Decide si hace falta descargar el mapa de una región, replicando la
/// lógica de `process()` en el `external_parser.py` original:
///
/// - Si el archivo local no existe, se descarga sí o sí.
/// - Si se obtuvo un fingerprint remoto y es distinto al guardado, se descarga.
/// - Si el fingerprint remoto no se pudo obtener (sin red, HEAD falló, etc.)
///   y el archivo local ya existe, se asume sin cambios -- no bloquea el build
///   por un problema de red puntual.
pub fn needs_download(
    local_file_exists: bool,
    cached: Option<&MapFingerprint>,
    remote: Option<&MapFingerprint>,
) -> bool {
    if !local_file_exists {
        return true;
    }
    match remote {
        Some(remote_fp) => Some(remote_fp) != cached,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fingerprint() -> MapFingerprint {
        MapFingerprint {
            etag: Some("\"abc123\"".to_string()),
            last_modified: Some("Wed, 15 Jan 2026 10:32:00 GMT".to_string()),
            content_length: Some("184320".to_string()),
        }
    }

    #[test]
    fn round_trip_save_and_load() {
        let dir = std::env::temp_dir().join(format!("sde-manifest-test-{}", std::process::id()));
        let maps_dir = dir.join("maps");

        let mut manifest = Manifest::new();
        manifest.insert("The Forge".to_string(), sample_fingerprint());

        save(&maps_dir, &manifest).expect("save debe funcionar");
        let loaded = load(&maps_dir);
        assert_eq!(loaded, manifest);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_missing_manifest_returns_empty() {
        let dir = std::env::temp_dir().join(format!("sde-manifest-missing-{}", std::process::id()));
        let manifest = load(&dir.join("maps"));
        assert!(manifest.is_empty());
    }

    #[test]
    fn needs_download_when_local_file_missing() {
        let fp = sample_fingerprint();
        assert!(needs_download(false, Some(&fp), Some(&fp)));
    }

    #[test]
    fn needs_download_when_fingerprint_changed() {
        let cached = sample_fingerprint();
        let mut remote = sample_fingerprint();
        remote.etag = Some("\"different\"".to_string());
        assert!(needs_download(true, Some(&cached), Some(&remote)));
    }

    #[test]
    fn no_download_when_fingerprint_unchanged() {
        let fp = sample_fingerprint();
        assert!(!needs_download(true, Some(&fp), Some(&fp)));
    }

    #[test]
    fn no_download_when_remote_check_failed_but_local_exists() {
        let cached = sample_fingerprint();
        assert!(!needs_download(true, Some(&cached), None));
    }

    #[test]
    fn download_when_no_cached_entry_yet() {
        let remote = sample_fingerprint();
        assert!(needs_download(true, None, Some(&remote)));
    }
}
