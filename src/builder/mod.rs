//! Lógica para (re)generar `sde.db` a partir del SDE oficial de CCP y los
//! mapas de dotlan.
//!
//! Todo este módulo vive detrás de la feature `builder` (deshabilitada por
//! default) para que un consumidor que solo *lee* `sde.db` -- como la app
//! egui principal del proyecto -- no arrastre reqwest/tokio/zip/etc.
//! Los binarios `sde` (CLI) y `sde-gui` habilitan esta feature y llaman a
//! las funciones de este módulo.

pub mod dotlan;
pub mod http;
pub mod manifest;
pub mod parser;
pub mod schema;
pub mod sde_index;

// Próximos submódulos (aún no portados desde el prototipo en Python):
// pub mod extract;    // descompresión del zip del SDE, preservando maps/
//
// `schema` (DDL STRICT) ya está portado -- ver builder::schema::create_schema().
// `parser` (escritura de datos) ya está portado por completo -- 14
// funciones, paridad total con parse_data() de Python. Ver el docstring
// de builder::parser para el detalle fase por fase.
// `dotlan` (datos comunitarios externos al SDE) ya está portado por
// completo -- DDL dinámico, poblado de listas estáticas, parseo de SVG
// (validado contra un mapa real de dotlan) y el orquestador de descarga
// con reintentos. Ver el docstring de builder::dotlan para el detalle.
// `sde_index` (chequeo de build number + descarga condicional del SDE)
// ya está portado -- ver el docstring de builder::sde_index. Falta
// `builder::extract` para descomprimir el zip descargado, y el
// orquestador de nivel superior que une todo esto (aún un stub en
// src/bin/cli.rs).

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
    /// Error al ejecutar una consulta SQL (creación de schema, inserts del
    /// parser, etc.).
    Sqlite(rusqlite::Error),
    /// Un registro del SDE no tiene la forma esperada (campo requerido
    /// ausente, o de un tipo distinto al esperado). No es un error de
    /// sintaxis JSON (`Json` ya cubre eso, cuando el archivo ni siquiera
    /// parsea) ni de E/S (`Io`) -- el archivo se leyó y parseó bien, el
    /// contenido simplemente no calza con lo que el parser necesita.
    Data(String),
}

impl std::fmt::Display for BuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderError::Io(err) => write!(f, "I/O error: {err}"),
            BuilderError::Json(err) => write!(f, "JSON error: {err}"),
            BuilderError::Http(err) => write!(f, "HTTP error: {err}"),
            BuilderError::HttpStatus { url, status } => {
                write!(f, "{url} responded with status {status}")
            },
            BuilderError::Sqlite(err) => write!(f, "SQLite error: {err}"),
            BuilderError::Data(message) => write!(f, "malformed SDE record: {message}"),
        }
    }
}

impl std::error::Error for BuilderError {}

impl From<std::io::Error> for BuilderError {
    fn from(err: std::io::Error) -> Self {
        BuilderError::Io(err)
    }
}

impl From<rusqlite::Error> for BuilderError {
    fn from(err: rusqlite::Error) -> Self {
        BuilderError::Sqlite(err)
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