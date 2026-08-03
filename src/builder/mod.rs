//! Lógica para (re)generar `sde.db` a partir del SDE oficial de CCP y los
//! mapas de dotlan.
//!
//! Todo este módulo vive detrás de la feature `builder` (deshabilitada por
//! default) para que un consumidor que solo *lee* `sde.db` -- como la app
//! egui principal del proyecto -- no arrastre reqwest/tokio/zip/etc.
//! Los binarios `sde` (CLI) y `sde-gui` habilitan esta feature y llaman a
//! las funciones de este módulo.

pub mod http;
pub mod manifest;

// Próximos submódulos (aún no portados desde el prototipo en Python):
// pub mod sde_index;  // parseo de latest.jsonl y comparación de build number
// pub mod dotlan;     // descarga + parseo de mapas SVG (roxmltree)
// pub mod schema;     // DDL STRICT y la escritura final a sqlite (rusqlite)

/// Errores del proceso de build. Sin `thiserror` a propósito: es el mismo
/// patrón "sin abstracción" que ya usa `SdeManager` (propaga los errores de
/// las crates de abajo tal cual con `?`), solo que aquí sí necesitamos
/// unificar varios tipos de error distintos (HTTP, IO, zip, JSON, sqlite).
#[derive(Debug)]
pub enum BuilderError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    /// El servidor respondió, pero con un status HTTP que no es 2xx.
    HttpStatus { url: String, status: u16 },
}

impl std::fmt::Display for BuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderError::Io(err) => write!(f, "I/O error: {err}"),
            BuilderError::Json(err) => write!(f, "JSON error: {err}"),
            BuilderError::Http(err) => write!(f, "HTTP error: {err}"),
            BuilderError::HttpStatus { url, status } => {
                write!(f, "{url} responded with status {status}")
            }
        }
    }
}

impl std::error::Error for BuilderError {}

impl From<std::io::Error> for BuilderError {
    fn from(err: std::io::Error) -> Self {
        BuilderError::Io(err)
    }
}

impl From<serde_json::Error> for BuilderError {
    fn from(err: serde_json::Error) -> Self {
        BuilderError::Json(err)
    }
}

impl From<reqwest::Error> for BuilderError {
    fn from(err: reqwest::Error) -> Self {
        BuilderError::Http(err)
    }
}