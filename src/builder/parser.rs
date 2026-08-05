//! Población de datos del SDE en las tablas creadas por [`super::schema`].
//!
//! Puerto (parcial, ver "Alcance" más abajo) de los métodos `_parse_*` de
//! `SdeParser` en el prototipo Python (`sde_parser.py`). Cada `_parse_*`
//! de Python corresponde aquí a una función `parse_*` que recibe la
//! conexión ya abierta (con el schema ya creado por
//! [`super::schema::create_schema`]) y el directorio donde viven los
//! archivos planos del SDE (`categories.jsonl`, `types.jsonl`, etc.).
//!
//! A diferencia de Python (donde `SdeParser` es una clase con estado
//! mutable en `self`), aquí las funciones son libres y sin estado propio;
//! el único estado que de verdad necesita compartirse entre dos de ellas
//! -- el id del grupo "Sun" y el mapeo `typeId -> starTypeId` que detecta
//! `parse_types()` -- se pasa explícitamente vía [`StarTypeState`].
//!
//! ## Alcance de este archivo (fase 1 + fase 2)
//!
//! Cubre las tablas "base" que el resto de entidades referencian por FK y
//! que no dependen de ninguna tabla de mapa: `invCategories`, `invGroups`,
//! `invTypes` (incluyendo la detección especial de tipos de estrella que
//! alimenta a `typeStar`), `races` (fase 1), `npcCorporations` y
//! `factions` + `factionRace` (fase 2). Equivalen a `_parse_categories`,
//! `_parse_groups`, `_parse_types`, `_parse_races`,
//! `_parse_npc_corporations` y `_parse_factions` en Python.
//!
//! Deliberadamente NO incluye todavía (quedan para una siguiente fase):
//! `mapRegions`, `mapConstellations`, `mapSolarSystems` (con la
//! proyección isométrica/dimétrica y el filtro
//! k-space/w-space/abyssal/void), `mapSystemGates`, `mapStars`,
//! `mapPlanets`, `mapMoons` ni `mapSystemConnections`.
//!
//! ## Formato de archivo soportado
//!
//! Solo JSONL (`SdeConfig.file_format == 'jsonl'` en el prototipo Python,
//! que además es el default). El soporte YAML del prototipo (la otra
//! rama de `_iter_records`) no está portado -- añadirlo implica sumar una
//! dependencia de parseo YAML nueva al crate, decisión que se dejó fuera
//! de esta fase a propósito.
//!
//! ## Desviaciones conocidas respecto al prototipo Python
//!
//! - `_parse_types()` declara un diccionario `process = {}` que nunca se
//!   llena (`process.get(...)` siempre da `None`), así que su chequeo
//!   `if process.get(...) is not None: pass` nunca se cumple y todo tipo
//!   termina insertándose de todos modos -- es código muerto. Este puerto
//!   no lo replica; el comportamiento observable es idéntico (se inserta
//!   cada tipo del archivo).
//! - Si el nombre de un tipo del grupo "Sun" no tiene al menos 3 tokens
//!   separados por espacio, Python lanza `IndexError` y aborta todo el
//!   proceso. Aquí, en cambio, ese tipo simplemente no se trata como
//!   estrella (no se inserta en `typeStar`) y el resto del archivo se
//!   sigue procesando con normalidad. Si se prefiere paridad estricta
//!   (abortar como en Python), avisar para cambiarlo a un
//!   `BuilderError::Data`.
//! - La extracción del color usa `strip_prefix('(')`/`strip_suffix(')')`
//!   en vez del slice ciego `[1:-1]` de Python (que quita el primer/último
//!   carácter sean o no paréntesis). Con datos bien formados el resultado
//!   es idéntico; con datos mal formados, esta versión es más tolerante.
//! - **Transacciones**: en Python, ningún `_parse_*` hace `commit()` --
//!   todo el pipeline corre en una única transacción implícita que recién
//!   se confirma en `SdeParser.close()`, al final de todo. Si algo falla
//!   a mitad de camino, nada queda persistido (rollback implícito al
//!   cerrar sin commit). Las funciones de este archivo, tal como están,
//!   NO envuelven sus inserts en una transacción explícita, así que cada
//!   `INSERT` se autocommitea de inmediato (modo por defecto de SQLite):
//!   si `parse_categories()` falla en el registro 50 de 100, los primeros
//!   49 quedan grabados igual, a diferencia de Python. `Connection::
//!   transaction()` de rusqlite exige `&mut Connection`, así que arreglar
//!   esto implica cambiar la firma de estas funciones -- se dejó
//!   pendiente a propósito para cuando exista el orquestador equivalente
//!   a `parse_data()` (que llame a todas las fases en secuencia dentro de
//!   una sola transacción, igual que Python), en vez de envolver cada
//!   función por separado ahora con una granularidad que no calzaría con
//!   el comportamiento real de Python de todos modos.
//! - `_parse_factions()` itera `faction.get('memberRaces', [])` sin
//!   validar sus elementos -- si alguno no fuera un entero, Python
//!   simplemente se lo pasaría a `cur.execute()` tal cual y fallaría (o
//!   no) según el driver. Aquí, [`parse_factions`] valida cada elemento y
//!   devuelve [`BuilderError::Data`] ante el primero que no sea entero,
//!   ya que de todos modos violaría `factionRace.raceId INTEGER NOT NULL`
//!   al insertar -- fallar temprano con un mensaje claro es preferible a
//!   un error de SQLite genérico más abajo.

use crate::builder::BuilderError;
use rusqlite::Connection;
use serde_json::Value;
use std::io::BufRead;
use std::path::Path;

/// Configuración mínima para esta fase del parser: por ahora, solo lo que
/// hace falta para localizar nombres (`_localized()` en Python). Los
/// flags de alcance de sistema solar (k-space/w-space/abyssal/void) y la
/// proyección isométrica/dimétrica se agregan cuando se porte
/// `_parse_solar_systems` en una fase futura.
#[derive(Debug, Clone)]
pub struct ParserConfig {
    /// Idioma a extraer de los campos `name`/`description` localizados
    /// (p. ej. `{"en": "Jita", "es": "Jita"}` -> `"Jita"`), con fallback a
    /// `"en"` si el idioma pedido no está. Default `"en"`, igual que
    /// `SdeConfig.language` en Python.
    pub language: String,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
        }
    }
}

/// Estado compartido entre [`parse_groups`] y [`parse_types`], equivalente
/// a `SdeParser._stars` (`DataBrigde`) en Python.
#[derive(Debug, Default)]
pub struct StarTypeState {
    /// `groupId` del grupo llamado exactamente `"Sun"`, una vez que
    /// [`parse_groups`] lo encuentra.
    pub sun_group_id: Option<i64>,
    /// `typeId` (de `invTypes`) -> `starTypeId` (de `typeStar`) para cada
    /// tipo de estrella insertado por [`parse_types`]. Lo va a necesitar
    /// `parse_stars()` en una fase futura del builder (no portada
    /// todavía).
    pub star_type_ids: std::collections::HashMap<i64, i64>,
}

// ---------------------------------------------------------------------
// Infraestructura compartida: lectura de archivos planos + helpers de
// extracción de campos, equivalentes a `_iter_records()` / `_localized()`
// en Python.
// ---------------------------------------------------------------------

/// Itera los registros de `<sde_directory>/<stem>.jsonl`, una línea (no
/// vacía) a la vez, como [`serde_json::Value`].
///
/// Equivalente a la rama `jsonl` de `_iter_records()` en Python. Cada
/// registro trae su propio campo `_key` (el id) por convención del nuevo
/// SDE -- a diferencia de la rama YAML de Python, no hace falta
/// inyectarlo aparte.
fn iter_jsonl_records(
    sde_directory: &Path,
    stem: &str,
) -> Result<impl Iterator<Item = Result<Value, BuilderError>>, BuilderError> {
    let path = sde_directory.join(format!("{stem}.jsonl"));
    let file = std::fs::File::open(&path)?;
    let reader = std::io::BufReader::new(file);
    Ok(reader.lines().filter_map(|line| match line {
        Ok(line) if line.trim().is_empty() => None,
        Ok(line) => Some(serde_json::from_str::<Value>(&line).map_err(BuilderError::Json)),
        Err(err) => Some(Err(BuilderError::Io(err))),
    }))
}

/// Extrae el string localizado en `config.language` de un campo tipo
/// `{"en": "...", "es": "...", ...}`, con fallback a `"en"`. Si `field` ya
/// es un string plano (no un objeto), se devuelve tal cual. Equivalente a
/// `_localized()` en Python.
fn localized<'a>(record: &'a Value, field: &str, config: &ParserConfig) -> Option<&'a str> {
    match record.get(field) {
        Some(Value::Object(map)) => map
            .get(config.language.as_str())
            .or_else(|| map.get("en"))
            .and_then(Value::as_str),
        Some(Value::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Extrae un campo entero requerido del registro. Equivalente a un acceso
/// tipo `dict[key]` en Python (que lanza `KeyError` si falta): si el campo
/// no está presente o no es numérico, esto es un error de datos
/// ([`BuilderError::Data`]), no un `None` silencioso.
fn required_i64(record: &Value, field: &str) -> Result<i64, BuilderError> {
    record.get(field).and_then(Value::as_i64).ok_or_else(|| {
        BuilderError::Data(format!(
            "registro sin campo requerido `{field}` (o no es un entero): {record}"
        ))
    })
}

/// Extrae un campo entero opcional. Equivalente a `dict.get(key)` en
/// Python (`None` si falta, sin error).
fn optional_i64(record: &Value, field: &str) -> Option<i64> {
    record.get(field).and_then(Value::as_i64)
}

/// Extrae un nombre localizado requerido (vía [`localized`]); si el campo
/// falta o no es un string/objeto localizable, es un error de datos. Las
/// columnas de nombre a las que alimenta esto (`categoryName`,
/// `groupName`, `typeName`, `raceName`) son todas `TEXT NOT NULL` en el
/// schema STRICT -- Python también fallaría aquí (con `IntegrityError` al
/// intentar insertar `NULL`), así que se trata igual de "duro" que
/// `required_i64` en vez de insertar silenciosamente una cadena vacía.
fn required_localized<'a>(
    record: &'a Value,
    field: &str,
    config: &ParserConfig,
) -> Result<&'a str, BuilderError> {
    localized(record, field, config).ok_or_else(|| {
        BuilderError::Data(format!(
            "registro sin campo `{field}` localizable en `{}`/`en`: {record}",
            config.language
        ))
    })
}

/// Extrae un campo booleano opcional. Equivalente a `dict.get(key)`.
fn optional_bool(record: &Value, field: &str) -> Option<bool> {
    record.get(field).and_then(Value::as_bool)
}

/// Extrae un campo de punto flotante opcional. Equivalente a
/// `dict.get(key)`.
fn optional_f64(record: &Value, field: &str) -> Option<f64> {
    record.get(field).and_then(Value::as_f64)
}

/// Extrae un campo string plano requerido (no localizado -- para campos
/// como `tickerName` que no traen variantes por idioma). Equivalente a un
/// acceso `dict[key]` en Python.
fn required_str<'a>(record: &'a Value, field: &str) -> Result<&'a str, BuilderError> {
    record.get(field).and_then(Value::as_str).ok_or_else(|| {
        BuilderError::Data(format!(
            "registro sin campo requerido `{field}` (o no es un string): {record}"
        ))
    })
}

/// Extrae un campo booleano requerido. Equivalente a un acceso
/// `dict[key]` en Python.
fn required_bool(record: &Value, field: &str) -> Result<bool, BuilderError> {
    record.get(field).and_then(Value::as_bool).ok_or_else(|| {
        BuilderError::Data(format!(
            "registro sin campo requerido `{field}` (o no es un booleano): {record}"
        ))
    })
}

/// Extrae un campo de punto flotante requerido. Equivalente a un acceso
/// `dict[key]` en Python.
fn required_f64(record: &Value, field: &str) -> Result<f64, BuilderError> {
    record.get(field).and_then(Value::as_f64).ok_or_else(|| {
        BuilderError::Data(format!(
            "registro sin campo requerido `{field}` (o no es un número): {record}"
        ))
    })
}

/// Extrae los ids de un array entero opcional -- vacío si el campo no
/// está o es `null`, igual que `faction.get('memberRaces', [])` en
/// Python. Si el campo SÍ está pero no es un array, o alguno de sus
/// elementos no es entero, es un error de datos (ver "Desviaciones
/// conocidas" en el docstring del módulo).
fn optional_i64_array(record: &Value, field: &str) -> Result<Vec<i64>, BuilderError> {
    match record.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_i64().ok_or_else(|| {
                    BuilderError::Data(format!("elemento no entero en el array `{field}`: {item}"))
                })
            })
            .collect(),
        Some(other) => Err(BuilderError::Data(format!(
            "campo `{field}` no es un array: {other}"
        ))),
    }
}

// ---------------------------------------------------------------------
// invCategories
// ---------------------------------------------------------------------

/// Puebla `invCategories` desde `<sde_directory>/categories.jsonl`.
/// Devuelve la cantidad de filas insertadas. Equivalente a
/// `_parse_categories()` en Python.
pub fn parse_categories(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_category = connection.prepare(
        "INSERT INTO invCategories (categoryId, categoryName, published) VALUES (?1, ?2, ?3)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "categories")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;
        let published = optional_bool(&record, "published");

        insert_category.execute(rusqlite::params![id, name, published])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// invGroups
// ---------------------------------------------------------------------

/// Puebla `invGroups` desde `<sde_directory>/groups.jsonl`. De paso,
/// detecta el grupo llamado exactamente `"Sun"` y guarda su id en
/// `state.sun_group_id` -- lo necesita [`parse_types`] para reconocer los
/// tipos de estrella. Devuelve la cantidad de filas insertadas.
/// Equivalente a `_parse_groups()` en Python.
pub fn parse_groups(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
    state: &mut StarTypeState,
) -> Result<usize, BuilderError> {
    let mut insert_group = connection.prepare(
        "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) \
         VALUES (?1, ?2, ?3, ?4)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "groups")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let category_id = required_i64(&record, "categoryID")?;
        let name = required_localized(&record, "name", config)?;
        let anchorable = optional_bool(&record, "anchorable");

        insert_group.execute(rusqlite::params![id, category_id, name, anchorable])?;

        if name == "Sun" {
            state.sun_group_id = Some(id);
        }

        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// invTypes (+ typeStar para los tipos de estrella)
// ---------------------------------------------------------------------

/// Inserta una fila en `typeStar` y devuelve el `starTypeId` que le
/// asignó SQLite. Equivalente a `add_star_type()` en Python (que hace lo
/// mismo: INSERT y luego un SELECT de vuelta por `typeId`, ya que
/// `typeStar.starTypeId` no tiene `AUTOINCREMENT` -- es un `ROWID` común
/// que igual se autoasigna).
fn add_star_type(
    connection: &Connection,
    type_id: i64,
    name: &str,
    color: &str,
) -> Result<i64, BuilderError> {
    connection.execute(
        "INSERT INTO typeStar (typeId, name, color) VALUES (?1, ?2, ?3)",
        rusqlite::params![type_id, name, color],
    )?;
    let star_type_id = connection.query_row(
        "SELECT starTypeId FROM typeStar WHERE typeId = ?1",
        rusqlite::params![type_id],
        |row| row.get(0),
    )?;
    Ok(star_type_id)
}

/// Puebla `invTypes` desde `<sde_directory>/types.jsonl`, y de paso
/// `typeStar` para cualquier tipo perteneciente al grupo "Sun" (detectado
/// por [`parse_groups`] vía `state.sun_group_id`). Devuelve la cantidad de
/// filas insertadas en `invTypes`. Equivalente a `_parse_types()` en
/// Python -- ver las "Desviaciones conocidas" en el docstring del módulo
/// para el manejo del código muerto `process` y de nombres de estrella
/// mal formados.
pub fn parse_types(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
    state: &mut StarTypeState,
) -> Result<usize, BuilderError> {
    let mut insert_type = connection.prepare(
        "INSERT INTO invTypes (typeId, groupId, typeName, iconId, published, volume) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "types")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let group_id = required_i64(&record, "groupID")?;
        let name = required_localized(&record, "name", config)?.to_string();
        let icon_id = optional_i64(&record, "iconID");
        let published = optional_bool(&record, "published");
        let volume = optional_f64(&record, "volume");

        insert_type.execute(rusqlite::params![id, group_id, name, icon_id, published, volume])?;

        if state.sun_group_id == Some(group_id) {
            let parts: Vec<&str> = name.split(' ').collect();
            if parts.len() >= 3 {
                let star_name = parts[1];
                let color_token = parts[2];
                let color = color_token
                    .strip_prefix('(')
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(color_token);
                let star_type_id = add_star_type(connection, id, star_name, color)?;
                state.star_type_ids.insert(id, star_type_id);
            }
            // Menos de 3 tokens: no se trata como estrella. Ver
            // "Desviaciones conocidas" en el docstring del módulo --
            // Python abortaría todo el proceso con IndexError aquí.
        }

        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// races
// ---------------------------------------------------------------------

/// Puebla `races` desde `<sde_directory>/races.jsonl`. Devuelve la
/// cantidad de filas insertadas. Equivalente a `_parse_races()` en
/// Python.
pub fn parse_races(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_race =
        connection.prepare("INSERT INTO races (raceId, raceName) VALUES (?1, ?2)")?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "races")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;

        insert_race.execute(rusqlite::params![id, name])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// npcCorporations
// ---------------------------------------------------------------------

/// Puebla `npcCorporations` desde `<sde_directory>/npcCorporations.jsonl`.
/// Requiere que `races` ya esté poblada si algún registro trae `raceID`
/// (FK `npcCorporations.raceId -> races.raceId`). Devuelve la cantidad de
/// filas insertadas. Equivalente a `_parse_npc_corporations()` en Python.
pub fn parse_npc_corporations(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_corp = connection.prepare(
        "INSERT INTO npcCorporations (corporationId, corporationName, tickerName, deleted, iconId, raceId) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "npcCorporations")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;
        let ticker = required_str(&record, "tickerName")?;
        let deleted = required_bool(&record, "deleted")?;
        let icon_id = optional_i64(&record, "iconID");
        let race_id = optional_i64(&record, "raceID");

        insert_corp.execute(rusqlite::params![id, name, ticker, deleted, icon_id, race_id])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// factions (+ factionRace)
// ---------------------------------------------------------------------

/// Puebla `factions` y `factionRace` desde `<sde_directory>/factions.jsonl`.
/// Requiere que `npcCorporations` ya esté poblada si algún registro trae
/// `corporationID` (FK `factions.corporationId -> npcCorporations.
/// corporationId`), y que `races` ya esté poblada para cualquier id en
/// `memberRaces` (FK `factionRace.raceId -> races.raceId`). Devuelve la
/// cantidad de facciones insertadas (no cuenta las filas de
/// `factionRace`). Equivalente a `_parse_factions()` en Python.
pub fn parse_factions(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_faction = connection.prepare(
        "INSERT INTO factions (factionId, factionName, iconId, sizeFactor, uniqueName, corporationId) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut insert_faction_race =
        connection.prepare("INSERT INTO factionRace (factionId, raceId) VALUES (?1, ?2)")?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "factions")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;
        let icon_id = required_i64(&record, "iconID")?;
        let size_factor = required_f64(&record, "sizeFactor")?;
        let unique_name = required_bool(&record, "uniqueName")?;
        let corporation_id = optional_i64(&record, "corporationID");
        let member_races = optional_i64_array(&record, "memberRaces")?;

        insert_faction.execute(rusqlite::params![
            id,
            name,
            icon_id,
            size_factor,
            unique_name,
            corporation_id
        ])?;

        for race_id in member_races {
            insert_faction_race.execute(rusqlite::params![id, race_id])?;
        }

        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Directorio temporal único con los archivos `.jsonl` dados (nombre
    /// -> contenido), borrado automáticamente al salir de scope. Mismo
    /// patrón que la fixture de `tests/manager.rs`.
    struct TempSdeDir {
        path: std::path::PathBuf,
    }

    impl TempSdeDir {
        fn new(test_name: &str, files: &[(&str, &str)]) -> Self {
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "sde_parser_test_{}_{}_{}",
                test_name,
                std::process::id(),
                id
            ));
            std::fs::create_dir_all(&path).expect("cannot create temp sde dir");
            for (name, content) in files {
                std::fs::write(path.join(name), content).expect("cannot write fixture file");
            }
            Self { path }
        }
    }

    impl Drop for TempSdeDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn parse_categories_inserts_rows() {
        let dir = TempSdeDir::new(
            "categories",
            &[(
                "categories.jsonl",
                "{\"_key\": 6, \"name\": {\"en\": \"Ship\"}, \"published\": true}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let count = parse_categories(&connection, &dir.path, &config).unwrap();
        assert_eq!(count, 1);

        let (name, published): (String, i64) = connection
            .query_row(
                "SELECT categoryName, published FROM invCategories WHERE categoryId = 6",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "Ship");
        assert_eq!(published, 1);
    }

    #[test]
    fn parse_races_inserts_rows() {
        let dir = TempSdeDir::new(
            "races",
            &[(
                "races.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n\
                 {\"_key\": 2, \"name\": {\"en\": \"Minmatar\"}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let count = parse_races(&connection, &dir.path, &config).unwrap();
        assert_eq!(count, 2);

        let name: String = connection
            .query_row("SELECT raceName FROM races WHERE raceId = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Caldari");
    }

    #[test]
    fn parse_groups_and_types_detect_sun_and_populate_typestar() {
        let dir = TempSdeDir::new(
            "groups_types",
            &[
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n\
                     {\"_key\": 7, \"categoryID\": 6, \"name\": {\"en\": \"Frigate\"}, \"anchorable\": false}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n\
                     {\"_key\": 588, \"groupID\": 7, \"name\": {\"en\": \"Rifter\"}, \"iconID\": 200, \"published\": true, \"volume\": 27289.5}\n",
                ),
            ],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        // invCategories(6) lo exige la FK de invGroups.categoryId.
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (6, 'Celestial', 1)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let mut state = StarTypeState::default();

        let groups = parse_groups(&connection, &dir.path, &config, &mut state).unwrap();
        assert_eq!(groups, 2);
        assert_eq!(state.sun_group_id, Some(6));

        let types = parse_types(&connection, &dir.path, &config, &mut state).unwrap();
        assert_eq!(types, 2);

        // El tipo del grupo "Sun" debe haber generado una fila en typeStar.
        assert_eq!(state.star_type_ids.len(), 1);
        let star_type_id = state.star_type_ids[&3000];
        let (name, color): (String, String) = connection
            .query_row(
                "SELECT name, color FROM typeStar WHERE starTypeId = ?1",
                rusqlite::params![star_type_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "G5");
        assert_eq!(color, "ffcc00");

        // "Rifter" (grupo Frigate, no Sun) no debe generar fila en typeStar.
        let total_star_types: i64 = connection
            .query_row("SELECT COUNT(*) FROM typeStar", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total_star_types, 1);
    }

    #[test]
    fn parse_categories_missing_required_key_errors() {
        let dir = TempSdeDir::new(
            "missing_key",
            &[("categories.jsonl", "{\"name\": {\"en\": \"Ship\"}}\n")],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_categories(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_categories_missing_name_errors() {
        // categoryName es TEXT NOT NULL en el schema STRICT; Python
        // también fallaría aquí (IntegrityError al insertar NULL) -- ver
        // el docstring de `required_localized`.
        let dir = TempSdeDir::new("missing_name", &[("categories.jsonl", "{\"_key\": 6}\n")]);
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_categories(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_categories_missing_file_errors() {
        let dir = TempSdeDir::new("missing_file", &[]);
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        // No se escribió categories.jsonl en absoluto.
        let result = parse_categories(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn localized_falls_back_to_english() {
        let config = ParserConfig {
            language: "fr".to_string(),
        };
        let record: Value = serde_json::from_str(r#"{"name": {"en": "Jita", "de": "Jita"}}"#).unwrap();
        // "fr" no está presente -> cae a "en".
        assert_eq!(localized(&record, "name", &config), Some("Jita"));
    }

    #[test]
    fn localized_uses_requested_language_when_present() {
        let config = ParserConfig {
            language: "de".to_string(),
        };
        let record: Value =
            serde_json::from_str(r#"{"name": {"en": "Jita", "de": "Jita (de)"}}"#).unwrap();
        assert_eq!(localized(&record, "name", &config), Some("Jita (de)"));
    }

    #[test]
    fn parse_npc_corporations_inserts_rows() {
        let dir = TempSdeDir::new(
            "npc_corporations",
            &[(
                "npcCorporations.jsonl",
                "{\"_key\": 1000004, \"name\": {\"en\": \"CBD Corporation\"}, \
                 \"tickerName\": \"CBD\", \"deleted\": false, \"iconID\": 500, \"raceID\": 1}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        // races(1) lo exige la FK de npcCorporations.raceId.
        connection
            .execute(
                "INSERT INTO races (raceId, raceName) VALUES (1, 'Caldari')",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();

        let count = parse_npc_corporations(&connection, &dir.path, &config).unwrap();
        assert_eq!(count, 1);

        let (name, ticker, deleted, icon_id, race_id): (String, String, i64, i64, i64) =
            connection
                .query_row(
                    "SELECT corporationName, tickerName, deleted, iconId, raceId \
                     FROM npcCorporations WHERE corporationId = 1000004",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .unwrap();
        assert_eq!(name, "CBD Corporation");
        assert_eq!(ticker, "CBD");
        assert_eq!(deleted, 0);
        assert_eq!(icon_id, 500);
        assert_eq!(race_id, 1);
    }

    #[test]
    fn parse_npc_corporations_missing_ticker_errors() {
        // tickerName es TEXT NOT NULL y se accede como campo requerido
        // (equivalente a corporation['tickerName'] en Python).
        let dir = TempSdeDir::new(
            "npc_corp_missing_ticker",
            &[(
                "npcCorporations.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"deleted\": false}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_npc_corporations(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_factions_inserts_faction_and_member_races() {
        let dir = TempSdeDir::new(
            "factions",
            &[(
                "factions.jsonl",
                "{\"_key\": 500001, \"name\": {\"en\": \"Caldari State\"}, \"iconID\": 600, \
                 \"sizeFactor\": 3.0, \"uniqueName\": true, \"corporationID\": 1000004, \
                 \"memberRaces\": [1]}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        // Prerrequisitos de FK: races(1) para factionRace, npcCorporations(1000004)
        // para factions.corporationId.
        connection
            .execute(
                "INSERT INTO races (raceId, raceName) VALUES (1, 'Caldari')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO npcCorporations \
                 (corporationId, corporationName, tickerName, deleted, iconId, raceId) \
                 VALUES (1000004, 'CBD Corporation', 'CBD', 0, 500, 1)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();

        let count = parse_factions(&connection, &dir.path, &config).unwrap();
        assert_eq!(count, 1);

        let (name, icon_id, size_factor, unique_name, corporation_id): (
            String,
            i64,
            f64,
            i64,
            i64,
        ) = connection
            .query_row(
                "SELECT factionName, iconId, sizeFactor, uniqueName, corporationId \
                 FROM factions WHERE factionId = 500001",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(name, "Caldari State");
        assert_eq!(icon_id, 600);
        assert_eq!(size_factor, 3.0);
        assert_eq!(unique_name, 1);
        assert_eq!(corporation_id, 1000004);

        let member_race: i64 = connection
            .query_row(
                "SELECT raceId FROM factionRace WHERE factionId = 500001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(member_race, 1);
    }

    #[test]
    fn parse_factions_without_member_races_inserts_faction_only() {
        // memberRaces ausente -> factionRace se queda vacía para esta
        // facción, sin error (equivalente a `faction.get('memberRaces', [])`).
        let dir = TempSdeDir::new(
            "factions_no_members",
            &[(
                "factions.jsonl",
                "{\"_key\": 500002, \"name\": {\"en\": \"Minmatar Republic\"}, \"iconID\": 601, \
                 \"sizeFactor\": 2.5, \"uniqueName\": true}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let count = parse_factions(&connection, &dir.path, &config).unwrap();
        assert_eq!(count, 1);

        let total_faction_race: i64 = connection
            .query_row("SELECT COUNT(*) FROM factionRace", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total_faction_race, 0);
    }

    #[test]
    fn parse_factions_with_non_integer_member_race_errors() {
        let dir = TempSdeDir::new(
            "factions_bad_members",
            &[(
                "factions.jsonl",
                "{\"_key\": 500003, \"name\": {\"en\": \"Bad Faction\"}, \"iconID\": 602, \
                 \"sizeFactor\": 1.0, \"uniqueName\": false, \"memberRaces\": [1, \"oops\"]}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_factions(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_factions_missing_size_factor_errors() {
        // sizeFactor es REAL NOT NULL y se accede como campo requerido
        // (equivalente a faction['sizeFactor'] en Python).
        let dir = TempSdeDir::new(
            "factions_missing_size_factor",
            &[(
                "factions.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"iconID\": 1, \"uniqueName\": true}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_factions(&connection, &dir.path, &config);
        assert!(result.is_err());
    }
}
