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
//! ## Alcance de este archivo (fase 1 a fase 8)
//!
//! Cubre las tablas "base" que el resto de entidades referencian por FK y
//! que no dependen de ninguna tabla de mapa: `invCategories`, `invGroups`,
//! `invTypes` (incluyendo la detección especial de tipos de estrella que
//! alimenta a `typeStar`), `races` (fase 1), `npcCorporations` y
//! `factions` + `factionRace` (fase 2). Equivalen a `_parse_categories`,
//! `_parse_groups`, `_parse_types`, `_parse_races`,
//! `_parse_npc_corporations` y `_parse_factions` en Python.
//!
//! La fase 3 suma las dos primeras tablas de mapa, sin la complejidad de
//! la proyección isométrica/dimétrica que trae `mapSolarSystems`:
//! `mapRegions` y `mapConstellations`. Equivalen a `_parse_regions` y
//! `_parse_constellations` en Python.
//!
//! La fase 4 suma `mapSolarSystems`, con el filtro de alcance
//! k-space/w-space/abyssal/void ([`system_in_scope`]) y la proyección
//! isométrica ([`isometric_projection_2d`]). Equivale a
//! `_parse_solar_systems()` en Python.
//!
//! La fase 5 suma `mapSystemGates` ([`parse_stargates`], condicional a
//! `config.with_gates`), equivalente a `_parse_stargates()` en Python --
//! ver su docstring para una nota importante sobre por qué esta fase en
//! particular necesita correr dentro de una transacción explícita (no es
//! solo una cuestión de atomicidad, como en el resto del pipeline).
//!
//! La fase 6 suma `mapStars` ([`parse_stars`]), equivalente a
//! `_parse_stars()` en Python. El shape exacto de `mapStars.jsonl` se
//! confirmó contra una muestra real de datos (no solo contra el código
//! Python, que en este punto traía una nota del propio autor advirtiendo
//! que el shape no estaba verificado) -- ver el docstring de
//! [`parse_stars`] para el detalle.
//!
//! La fase 7 suma `mapPlanets` ([`parse_planets`]), equivalente a
//! `_parse_planets()` en Python -- mismo caso que `mapStars`, shape
//! confirmado contra una muestra real de 68407 registros (ver el
//! docstring de [`parse_planets`]).
//!
//! La fase 8 suma `mapMoons` ([`parse_moons`], condicional a
//! `config.with_moons`), equivalente a `_parse_moons()` en Python. A
//! diferencia de `mapStars`/`mapPlanets`, acá NO hubo una muestra real
//! disponible para verificar (el archivo real pesa más de 200 MiB) -- el
//! puerto se basa únicamente en el código Python, cuyo docstring SÍ
//! confirma la lista de campos. Ver el docstring de [`parse_moons`] para
//! el detalle de qué queda como inferencia (no verificación) en esta
//! fase.
//!
//! Deliberadamente NO incluye todavía (queda para una siguiente fase):
//! `mapSystemConnections`.
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
//!   cerrar sin commit). Las funciones `parse_*` individuales de este
//!   archivo, llamadas sueltas, NO envuelven sus inserts en una
//!   transacción explícita (autocommit por INSERT, modo por defecto de
//!   SQLite) -- solo [`parse_data`], el orquestador, envuelve las 6 fases
//!   en una única transacción real (`Connection::transaction()`), con el
//!   mismo comportamiento de "todo o nada" que Python. Si se llama a
//!   `parse_categories()` (u otra función individual) directamente, fuera
//!   de `parse_data`, sigue aplicando la falta de atomicidad descrita
//!   arriba -- para tener la garantía de Python hay que pasar siempre por
//!   `parse_data`.
//! - `_parse_factions()` itera `faction.get('memberRaces', [])` sin
//!   validar sus elementos -- si alguno no fuera un entero, Python
//!   simplemente se lo pasaría a `cur.execute()` tal cual y fallaría (o
//!   no) según el driver. Aquí, [`parse_factions`] valida cada elemento y
//!   devuelve [`BuilderError::Data`] ante el primero que no sea entero,
//!   ya que de todos modos violaría `factionRace.raceId INTEGER NOT NULL`
//!   al insertar -- fallar temprano con un mensaje claro es preferible a
//!   un error de SQLite genérico más abajo.
//! - `_parse_regions()` lee `region.get('nebulaID')` como opcional, pero
//!   `mapRegions.nebula` es `INTEGER NOT NULL` -- si faltara, Python
//!   fallaría igual al insertar (constraint NOT NULL). Aquí,
//!   [`parse_regions`] lo trata como requerido (mismo criterio que
//!   `name` en fase 1): falla con [`BuilderError::Data`] y un mensaje
//!   claro en vez de dejar que SQLite lo rechace más abajo.
//! - `_parse_regions()`/`_parse_constellations()` también arman
//!   `self._region_names`/`self._constellation_names`, pero solo para
//!   componer el texto de la barra de progreso en consola (`print(...,
//!   end="\r")`) -- no afectan ningún dato insertado. Este puerto no
//!   replica esa caché, ya que no hay un equivalente de progreso en
//!   consola todavía en ninguna función de este archivo.
//! - `_parse_constellations()` calcula el id como
//!   `element['constellationID'] if 'constellationID' in element else
//!   element['_key']`. Python distingue "la clave está presente" (con
//!   `in`) de "el valor es válido"; si `constellationID` estuviera
//!   presente pero fuera `null` o de otro tipo, Python igual lo usaría
//!   (y probablemente fallaría más abajo al insertar). [`parse_constellations`]
//!   en cambio cae a `_key` en ambos casos (ausente o presente-pero-no-entero)
//!   -- más tolerante, mismo resultado con datos bien formados.
//! - `_parse_solar_systems()` en Python soporta tres algoritmos para
//!   `projX`/`projY`/`projZ` según `self._config.projection_algorithm`:
//!   `'isometric'` (`calculate_isometric_projection()`), `'dimetric'`
//!   (`calculate_dimetric_projection()`, no portado) y cualquier otro
//!   valor (passthrough crudo de `position.x/y/z` sin transformar,
//!   tampoco portado). Esas tres columnas ya no existen en el schema (ver
//!   más abajo, "projX/Y/Z eliminadas"), así que esta rama de Python no
//!   se porta en absoluto -- ni siquiera el caso `'isometric'` que sí se
//!   portaba antes.
//! - **`projX`/`projY`/`projZ` eliminadas del schema**: guardaban una
//!   proyección 2D del centro del sistema calculada localmente (vía
//!   [`isometric_projection_2d`]), separada de `position2DX`/
//!   `position2DY` (la proyección 2D que ya trae CCP precalculada). Como
//!   ambas representan el mismo concepto, mantener las dos era
//!   redundante -- se decidió explícitamente eliminar `projX/Y/Z` y
//!   migrar todo a `position2DX`/`position2DY` (incluyendo las queries de
//!   `SdeManager` en `src/lib.rs`, que antes leían `projX`/`projZ`).
//!   [`isometric_projection_2d`] en sí sigue existiendo -- solo perdió
//!   este caso de uso; sigue siendo lo que calcula `position2DX`/
//!   `position2DY` cuando `config.force_isometric_position_2d` está
//!   activo.
//! - `self._system_names` en Python (poblado en `_parse_solar_systems`,
//!   junto a `_systems_in_scope`) nunca se lee en ningún lado del
//!   prototipo -- ni siquiera para un print de progreso, a diferencia de
//!   `_region_names`/`_constellation_names`. Es dead code puro. Este
//!   puerto no lo replica.

use crate::builder::BuilderError;
use rusqlite::Connection;
use serde_json::Value;
use std::io::BufRead;
use std::path::Path;

/// Configuración para el parser. Cubre lo que hace falta para localizar
/// nombres (`_localized()` en Python) y el cálculo isométrico opcional de
/// `position2DX`/`position2DY` (ver [`ProjectedAxis`]/
/// [`isometric_projection_2d`]). Los flags de alcance de sistema solar
/// (k-space/w-space/abyssal/void) y el algoritmo de proyección dimétrico
/// (`calculate_dimetric_projection()` en Python, no portado) se agregan
/// cuando se porte `_parse_solar_systems` en una fase futura.
#[derive(Debug, Clone)]
pub struct ParserConfig {
    /// Idioma a extraer de los campos `name`/`description` localizados
    /// (p. ej. `{"en": "Jita", "es": "Jita"}` -> `"Jita"`), con fallback a
    /// `"en"` si el idioma pedido no está. Default `"en"`, igual que
    /// `SdeConfig.language` en Python.
    pub language: String,
    /// Si es `true`, `position2DX`/`position2DY` se calculan siempre
    /// localmente vía [`isometric_projection_2d`], **ignorando** el campo
    /// `position2D` que ya trae CCP en el SDE reworkeado -- en vez de
    /// usar directamente el valor que CCP provee precalculado (que es el
    /// comportamiento por default, `false`).
    ///
    /// Nota: todavía no hay ningún `parse_*` que puebla `mapSolarSystems`
    /// (queda para una fase futura, ver el docstring del módulo), así que
    /// por ahora este flag no tiene ningún efecto observable -- es la
    /// pieza de configuración que esa función futura va a consultar.
    pub force_isometric_position_2d: bool,
    /// Eje que se colapsa en el cálculo de [`isometric_projection_2d`]
    /// cuando `force_isometric_position_2d` está activo (sin efecto si no
    /// lo está). Default [`ProjectedAxis::Y`], igual que
    /// `SdeConfig.projected_axis = 1` en Python (`0` para X, `1` para Y,
    /// `2` para Z).
    pub isometric_projected_axis: ProjectedAxis,
    /// Incluir sistemas k-space (sin `wormholeClassID`). Default `true`,
    /// igual que `SdeConfig.map_kspace` en Python.
    pub map_kspace: bool,
    /// Incluir sistemas de wormhole space. Default `true`, igual que
    /// `SdeConfig.map_wspace` en Python.
    pub map_wspace: bool,
    /// Incluir sistemas de abyssal deadspace. Default `true`, igual que
    /// `SdeConfig.map_abyssal` en Python.
    pub map_abyssal: bool,
    /// Incluir sistemas "void". Default `false`, igual que
    /// `SdeConfig.map_void` en Python. Ver [`system_in_scope`] para la
    /// nota sobre por qué, hoy, `map_wspace`/`map_abyssal`/`map_void`
    /// terminan gateando sobre el mismo chequeo.
    pub map_void: bool,
    /// Si es `false`, [`parse_data`] omite la fase de stargates
    /// ([`parse_stargates`]) por completo -- no la llama en absoluto, no
    /// solo filtra sus resultados. Default `true`, igual que
    /// `SdeConfig.with_gates` en Python.
    pub with_gates: bool,
    /// Si es `false`, [`parse_data`] omite la fase de lunas
    /// ([`parse_moons`]) por completo -- no la llama en absoluto. Default
    /// `true`, igual que `SdeConfig.with_moons` en Python.
    pub with_moons: bool,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            force_isometric_position_2d: false,
            isometric_projected_axis: ProjectedAxis::default(),
            map_kspace: true,
            map_wspace: true,
            map_abyssal: true,
            map_void: false,
            with_gates: true,
            with_moons: true,
        }
    }
}

/// Eje que se "colapsa" (se descarta) al calcular una proyección
/// isométrica 2D de un punto 3D -- equivalente al parámetro entero
/// `projected_axis` de Python (`0` para X, `1` para Y, `2` para Z).
///
/// En la fórmula de Python, el componente del eje colapsado siempre
/// queda en `0.0` en la tupla de salida de 3 elementos; como ese valor
/// nunca aporta información, [`isometric_projection_2d`] lo omite
/// directamente y devuelve solo los dos componentes restantes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectedAxis {
    X,
    /// Default -- coincide con `SdeConfig.projected_axis = 1` en Python.
    #[default]
    Y,
    Z,
}

/// Proyección isométrica 2D de un punto 3D, colapsando `axis`. Puerto
/// exacto de `calculate_isometric_projection()` en Python (mismas
/// fórmulas, mismo eje colapsado); a diferencia de Python, que siempre
/// devuelve una tupla de 3 con un `0.0` de relleno en el eje colapsado,
/// esta función devuelve directamente los dos componentes no nulos como
/// `(x2d, y2d)`.
///
/// Fórmulas (de <https://www.compuphase.com/axometr.htm>, según el
/// comentario original en Python):
/// - eje Z colapsado: `(x - z, y + (x + z) / 2)`
/// - eje Y colapsado: `(x - y, z + (x + y) / 2)`
/// - eje X colapsado: `(y - x, z + (y + x) / 2)`
pub fn isometric_projection_2d(x: f64, y: f64, z: f64, axis: ProjectedAxis) -> (f64, f64) {
    match axis {
        ProjectedAxis::Z => (x - z, y + (x + z) / 2.0),
        ProjectedAxis::Y => (x - y, z + (x + y) / 2.0),
        ProjectedAxis::X => (y - x, z + (y + x) / 2.0),
    }
}

/// Decide si un sistema solar debe importarse, según los flags
/// `map_kspace`/`map_wspace`/`map_abyssal`/`map_void`. Puerto exacto de
/// `_system_in_scope()` en Python.
///
/// El SDE reworkeado ya no separa k-space/w-space/abyssal/void por
/// directorio como el viejo; el único discriminador confirmado en el
/// propio registro es `wormholeClassID` (solo presente en sistemas que
/// NO son k-space). CCP no expone un flag más fino para distinguir
/// abyssal de void a este nivel, así que -- igual que en Python --
/// `map_wspace`/`map_abyssal`/`map_void` hoy comparten el mismo chequeo
/// ("¿tiene `wormholeClassID`?").
fn system_in_scope(wormhole_class_id: Option<i64>, config: &ParserConfig) -> bool {
    match wormhole_class_id {
        None => config.map_kspace,
        Some(_) => config.map_wspace || config.map_abyssal || config.map_void,
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

/// Ids de sistema solar que pasaron el filtro de [`system_in_scope`],
/// poblado por [`parse_solar_systems`]. Equivalente a
/// `self._systems_in_scope` en Python, que usan `_parse_stargates`,
/// `_parse_stars`, `_parse_planets` y `_parse_moons` para filtrar sus
/// propios registros por `solarSystemID` -- ninguna de esas cuatro está
/// portada todavía, pero este estado es lo que van a necesitar cuando se
/// porten.
#[derive(Debug, Default)]
pub struct SystemScopeState {
    pub systems_in_scope: std::collections::HashSet<i64>,
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

/// Extrae `record["position"]["x"/"y"/"z"]` como `(f64, f64, f64)`.
/// Equivalente al acceso anidado `record['position']['x']` (etc.) en
/// Python -- ambos niveles son requeridos (`dict[key]`, no `.get()`); si
/// falta `position` o cualquiera de sus tres componentes, es un error de
/// datos.
fn required_position(record: &Value) -> Result<(f64, f64, f64), BuilderError> {
    let position = record.get("position").ok_or_else(|| {
        BuilderError::Data(format!("registro sin campo requerido `position`: {record}"))
    })?;
    let x = required_f64(position, "x")?;
    let y = required_f64(position, "y")?;
    let z = required_f64(position, "z")?;
    Ok((x, y, z))
}

/// Extrae `record[outer][inner]` como `i64` requerido. Equivalente al
/// acceso anidado `record[outer][inner]` en Python (ambos niveles son
/// `dict[key]`, no `.get()`) -- usado para `destination.stargateID`/
/// `destination.solarSystemID` en [`parse_stargates`].
fn required_nested_i64(record: &Value, outer: &str, inner: &str) -> Result<i64, BuilderError> {
    let outer_val = record.get(outer).ok_or_else(|| {
        BuilderError::Data(format!("registro sin campo requerido `{outer}`: {record}"))
    })?;
    required_i64(outer_val, inner)
}

/// Extrae un campo entero opcional que puede venir en el nivel superior
/// del registro o anidado bajo `nested_field` (p. ej. `statistics`), con
/// el nivel superior con prioridad. Aproxima el patrón Python
/// `record.get(field, nested.get(field))` (donde `nested =
/// record.get(nested_field) or {}`), usado en `_parse_stars()` para
/// `radius`/`locked` -- con una diferencia menor: Python distingue "la
/// clave está pero es `null`" (no cae al nested) de "la clave no está"
/// (sí cae); acá ambos casos caen al nested por igual, ya que
/// `optional_i64` no distingue "ausente" de "presente pero de tipo
/// incorrecto/null".
fn optional_i64_with_nested_fallback(record: &Value, field: &str, nested_field: &str) -> Option<i64> {
    optional_i64(record, field)
        .or_else(|| record.get(nested_field).and_then(|nested| optional_i64(nested, field)))
}

/// Igual que [`optional_i64_with_nested_fallback`], pero para campos
/// booleanos (p. ej. `locked`).
fn optional_bool_with_nested_fallback(
    record: &Value,
    field: &str,
    nested_field: &str,
) -> Option<bool> {
    optional_bool(record, field)
        .or_else(|| record.get(nested_field).and_then(|nested| optional_bool(nested, field)))
}

/// Igual que [`optional_i64_with_nested_fallback`], pero para campos de
/// punto flotante -- usado para `mapPlanets.radius` (columna `REAL`, a
/// diferencia de `mapStars.radius`, que es `INTEGER`).
fn optional_f64_with_nested_fallback(record: &Value, field: &str, nested_field: &str) -> Option<f64> {
    optional_f64(record, field)
        .or_else(|| record.get(nested_field).and_then(|nested| optional_f64(nested, field)))
}

/// Extrae un campo string plano opcional. Equivalente a `dict.get(key)`
/// en Python (`None` si falta, sin error).
fn optional_str<'a>(record: &'a Value, field: &str) -> Option<&'a str> {
    record.get(field).and_then(Value::as_str)
}

/// Extrae `record[outer][inner]` como `f64`, devolviendo `None` si falta
/// cualquiera de los dos niveles (o no es numérico). Equivalente al
/// patrón `outer_val = record.get(outer); outer_val.get(inner) if
/// outer_val else None` en Python -- usado para `position2D.x`/`.y`, que
/// a diferencia de `position` (ver [`required_position`]) es opcional en
/// ambos niveles.
fn optional_nested_f64(record: &Value, outer: &str, inner: &str) -> Option<f64> {
    record.get(outer)?.get(inner).and_then(Value::as_f64)
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

// ---------------------------------------------------------------------
// mapRegions
// ---------------------------------------------------------------------

/// Puebla `mapRegions` desde `<sde_directory>/mapRegions.jsonl`. Devuelve
/// la cantidad de filas insertadas. Equivalente a `_parse_regions()` en
/// Python.
///
/// `maxProjX`/`maxProjY` no se incluyen en el INSERT: el DDL les da
/// `DEFAULT(0.0)` y Python tampoco los especifica en su propia query, así
/// que SQLite aplica ese default automáticamente en ambos casos.
pub fn parse_regions(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_region = connection.prepare(
        "INSERT INTO mapRegions (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapRegions")? {
        let record = record?;
        let id = required_i64(&record, "_key")?;
        let name = required_localized(&record, "name", config)?;
        let faction_id = optional_i64(&record, "factionID");
        let nebula = required_i64(&record, "nebulaID")?;
        let wormhole_class_id = optional_i64(&record, "wormholeClassID");
        let (center_x, center_y, center_z) = required_position(&record)?;

        insert_region.execute(rusqlite::params![
            id,
            name,
            faction_id,
            center_x,
            center_y,
            center_z,
            nebula,
            wormhole_class_id
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapConstellations
// ---------------------------------------------------------------------

/// Puebla `mapConstellations` desde
/// `<sde_directory>/mapConstellations.jsonl`. Requiere que `mapRegions`
/// ya esté poblada (FK `mapConstellations.regionId -> mapRegions.
/// regionId`). Devuelve la cantidad de filas insertadas. Equivalente a
/// `_parse_constellations()` en Python.
///
/// El id preferido es `constellationID` si el registro lo trae; si no,
/// cae a `_key` -- replica el
/// `element['constellationID'] if 'constellationID' in element else
/// element['_key']` de Python (ver "Desviaciones conocidas" en el
/// docstring del módulo para el matiz de cuándo difiere).
pub fn parse_constellations(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<usize, BuilderError> {
    let mut insert_constellation = connection.prepare(
        "INSERT INTO mapConstellations (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapConstellations")? {
        let record = record?;
        let id = match optional_i64(&record, "constellationID") {
            Some(id) => id,
            None => required_i64(&record, "_key")?,
        };
        let name = required_localized(&record, "name", config)?;
        let region_id = required_i64(&record, "regionID")?;
        let (center_x, center_y, center_z) = required_position(&record)?;

        insert_constellation.execute(rusqlite::params![
            id, name, region_id, center_x, center_y, center_z
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapSolarSystems
// ---------------------------------------------------------------------

/// Puebla `mapSolarSystems` desde
/// `<sde_directory>/mapSolarSystems.jsonl`, filtrando por
/// [`system_in_scope`] y acumulando los ids que pasan el filtro en
/// `state.systems_in_scope`. Requiere que `mapConstellations` ya esté
/// poblada (FK `mapSolarSystems.constellationId -> mapConstellations.
/// constellationId`). Devuelve la cantidad de filas insertadas (los
/// sistemas fuera de alcance NO cuentan). Equivalente a
/// `_parse_solar_systems()` en Python.
///
/// `projX`/`projY`/`projZ` ya no existen en el schema (se eliminaron: el
/// único propósito real de esas columnas era guardar una proyección 2D
/// del centro del sistema, y eso es exactamente lo que ya hace
/// `position2DX`/`position2DY` -- mantener ambas era redundante). Ver
/// `schema.sql` y `SdeManager` en `src/lib.rs`, que se migró para leer
/// `position2DX`/`position2DY` en vez de `projX`/`projZ`.
///
/// `position2DX`/`position2DY` usan el `position2D` que ya trae CCP
/// precalculado, salvo que `config.force_isometric_position_2d` esté
/// activo -- en cuyo caso se recalculan siempre vía
/// [`isometric_projection_2d`] (según `config.isometric_projected_axis`),
/// **ignorando** el valor de CCP, tal como se decidió explícitamente para
/// este flag (ver su docstring en [`ParserConfig`]).
pub fn parse_solar_systems(
    connection: &Connection,
    sde_directory: &Path,
    config: &ParserConfig,
    state: &mut SystemScopeState,
) -> Result<usize, BuilderError> {
    let mut insert_system = connection.prepare(
        "INSERT INTO mapSolarSystems (solarSystemId, solarSystemName, constellationId, \
         corridor, fringe, hub, international, luminosity, radius, centerX, centerY, centerZ, \
         regional, security, securityClass, position2DX, position2DY) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapSolarSystems")? {
        let record = record?;
        let system_id = required_i64(&record, "_key")?;
        let wormhole_class_id = optional_i64(&record, "wormholeClassID");
        if !system_in_scope(wormhole_class_id, config) {
            continue;
        }
        state.systems_in_scope.insert(system_id);

        let name = required_localized(&record, "name", config)?;
        let constellation_id = required_i64(&record, "constellationID")?;
        let corridor = optional_bool(&record, "corridor");
        let fringe = optional_bool(&record, "fringe");
        let hub = optional_bool(&record, "hub");
        let international = optional_bool(&record, "international");
        let luminosity = optional_f64(&record, "luminosity");
        let radius = required_f64(&record, "radius")?;
        let (center_x, center_y, center_z) = required_position(&record)?;

        let regional = optional_bool(&record, "regional");
        let security = required_f64(&record, "securityStatus")?;
        let security_class = optional_str(&record, "securityClass");

        let (position_2d_x, position_2d_y) = if config.force_isometric_position_2d {
            let (x2d, y2d) = isometric_projection_2d(
                center_x,
                center_y,
                center_z,
                config.isometric_projected_axis,
            );
            (Some(x2d), Some(y2d))
        } else {
            (
                optional_nested_f64(&record, "position2D", "x"),
                optional_nested_f64(&record, "position2D", "y"),
            )
        };

        insert_system.execute(rusqlite::params![
            system_id,
            name,
            constellation_id,
            corridor,
            fringe,
            hub,
            international,
            luminosity,
            radius,
            center_x,
            center_y,
            center_z,
            regional,
            security,
            security_class,
            position_2d_x,
            position_2d_y,
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapSystemGates
// ---------------------------------------------------------------------

/// Puebla `mapSystemGates` desde `<sde_directory>/mapStargates.jsonl`
/// (el archivo se llama `mapStargates`, aunque la tabla destino se llame
/// `mapSystemGates` -- así lo nombra el propio SDE). Filtra por
/// `state.systems_in_scope` (poblado por [`parse_solar_systems`]): un
/// gate cuyo `solarSystemID` no esté en ese set se omite -- mismo
/// criterio que `gate['solarSystemID'] not in self._systems_in_scope` en
/// Python. Requiere que `mapSolarSystems`/`invTypes` ya estén pobladas
/// (FKs). Devuelve la cantidad de filas insertadas. Equivalente a
/// `_parse_stargates()` en Python.
///
/// # Importante: requiere una transacción explícita
///
/// `mapSystemGates.destinationGateId` referencia otra fila de la MISMA
/// tabla (`systemGateId`), declarada `DEFERRABLE INITIALLY DEFERRED` en
/// el schema -- eso le permite a SQLite postergar la validación de esa FK
/// hasta el `COMMIT` de la transacción, en vez de exigir que el gate
/// destino ya exista en el momento exacto del INSERT. Esto importa porque
/// los stargates suelen venir en pares que se referencian mutuamente (el
/// gate de A apunta al de B, y viceversa), así que sea cual sea el orden
/// del archivo, el primero de los dos en insertarse necesariamente
/// referencia a uno que todavía no existe.
///
/// Verificado empíricamente (sqlite3 con `isolation_level=None`, que
/// replica el modo autocommit real de SQLite/rusqlite): insertar ese
/// primer gate **fuera** de una transacción explícita falla con
/// `FOREIGN KEY constraint failed` -- en modo autocommit cada `INSERT` es
/// su propia transacción implícita, así que la validación diferida se
/// dispara igual, de inmediato, al cerrarse esa transacción de una sola
/// sentencia. Envuelto en una transacción explícita (`BEGIN`/`COMMIT`),
/// en cambio, ambos INSERTs se resuelven correctamente porque la
/// validación se pospone hasta el `COMMIT` final, para cuando los dos
/// gates ya existen.
///
/// En la práctica esto significa que llamar a esta función suelta (fuera
/// de [`parse_data`], sin pasar por `Connection::transaction()`) no solo
/// pierde la garantía de atomicidad de "todo o nada" que ya se documentó
/// para el resto del pipeline (ver "Transacciones" en el docstring del
/// módulo) -- acá puede hacer fallar la inserción de datos perfectamente
/// válidos, solo por el orden en que aparecen en el archivo.
pub fn parse_stargates(
    connection: &Connection,
    sde_directory: &Path,
    state: &SystemScopeState,
) -> Result<usize, BuilderError> {
    let mut insert_gate = connection.prepare(
        "INSERT INTO mapSystemGates (systemGateId, solarSystemId, typeId, \
         positionX, positionY, positionZ, destinationGateId, destinationSystemId) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapStargates")? {
        let record = record?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        if !state.systems_in_scope.contains(&solar_system_id) {
            continue;
        }

        let id = required_i64(&record, "_key")?;
        let type_id = required_i64(&record, "typeID")?;
        let (pos_x, pos_y, pos_z) = required_position(&record)?;
        let destination_gate_id = required_nested_i64(&record, "destination", "stargateID")?;
        let destination_system_id = required_nested_i64(&record, "destination", "solarSystemID")?;

        insert_gate.execute(rusqlite::params![
            id,
            solar_system_id,
            type_id,
            pos_x,
            pos_y,
            pos_z,
            destination_gate_id,
            destination_system_id,
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapStars
// ---------------------------------------------------------------------

/// Puebla `mapStars` desde `<sde_directory>/mapStars.jsonl`, filtrando
/// por `state.systems_in_scope` (poblado por [`parse_solar_systems`]).
/// Requiere que [`parse_types`] ya haya corrido -- necesita
/// `star_state.star_type_ids`, el mapeo `typeId -> starTypeId` -- y que
/// `mapSolarSystems`/`typeStar` ya estén pobladas (FKs). Devuelve la
/// cantidad de filas insertadas. Equivalente a `_parse_stars()` en
/// Python.
///
/// Confirmado contra una muestra real de `mapStars.jsonl` (8089
/// registros, EVE Online, agosto 2026): `radius` siempre viene en el
/// nivel superior como entero (nunca hace falta el fallback anidado a
/// `statistics.radius`), `statistics` siempre está presente, y `locked`
/// **nunca** aparece -- ni en el nivel superior ni dentro de
/// `statistics` -- así que en la práctica esa columna siempre sale
/// `NULL`. El fallback anidado (ver [`optional_i64_with_nested_fallback`]/
/// [`optional_bool_with_nested_fallback`]) se deja igual, fielmente
/// portado desde Python, por si otra versión del SDE sí llega a traerlo.
///
/// # Desviación de Python: `starTypeId` no encontrado
///
/// Python resuelve el tipo de estrella con
/// `self._stars.entity_type.get(star['typeID'], star['typeID'])`: si el
/// `typeID` de la estrella no está en el mapa (es decir, `_parse_types()`
/// no lo detectó como perteneciente al grupo "Sun"), usa el `typeID`
/// CRUDO como si fuera un `starTypeId` -- casi seguro violando la FK
/// `mapStars.starTypeId -> typeStar.starTypeId` al insertar, ya que son
/// secuencias de ids completamente distintas (una es `invTypes.typeId`,
/// la otra un `ROWID` autoasignado de `typeStar`). Acá, en cambio, no
/// encontrar el `typeId` en el mapa es un [`BuilderError::Data`] directo
/// -- mismo criterio que el resto del archivo: fallar temprano con un
/// mensaje claro en vez de dejar que SQLite rechace un valor que de
/// todos modos iba a ser inválido.
pub fn parse_stars(
    connection: &Connection,
    sde_directory: &Path,
    state: &SystemScopeState,
    star_state: &StarTypeState,
) -> Result<usize, BuilderError> {
    let mut insert_star = connection.prepare(
        "INSERT INTO mapStars (starId, solarSystemId, locked, radius, starTypeId) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapStars")? {
        let record = record?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        if !state.systems_in_scope.contains(&solar_system_id) {
            continue;
        }

        let star_id = required_i64(&record, "_key")?;
        let locked = optional_bool_with_nested_fallback(&record, "locked", "statistics");
        let radius = optional_i64_with_nested_fallback(&record, "radius", "statistics");
        let type_id = required_i64(&record, "typeID")?;
        let star_type_id = star_state.star_type_ids.get(&type_id).copied().ok_or_else(|| {
            BuilderError::Data(format!(
                "estrella {star_id}: typeId {type_id} no está en star_type_ids \
                 (parse_types() no lo detectó como tipo de estrella)"
            ))
        })?;

        insert_star.execute(rusqlite::params![
            star_id,
            solar_system_id,
            locked,
            radius,
            star_type_id
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapPlanets
// ---------------------------------------------------------------------

/// Puebla `mapPlanets` desde `<sde_directory>/mapPlanets.jsonl`, filtrando
/// por `state.systems_in_scope` (poblado por [`parse_solar_systems`]).
/// Requiere que `mapSolarSystems`/`invTypes` ya estén pobladas (FKs).
/// Devuelve la cantidad de filas insertadas. Equivalente a
/// `_parse_planets()` en Python.
///
/// Confirmado contra una muestra real de `mapPlanets.jsonl` (68407
/// registros, EVE Online, agosto 2026):
/// - `celestialIndex`, `position`, `typeID` y `solarSystemID` están
///   presentes en el 100% de los registros -- a diferencia de Python
///   (que lee `celestialIndex` con `.get()`, opcional), acá se tratan
///   como requeridos ([`required_i64`]/[`required_position`]), mismo
///   criterio usado en todo este archivo para columnas `NOT NULL`
///   (`mapPlanets.planetaryIndex` lo es) cuando la fuente real confirma
///   que el dato siempre está: falla temprano con un mensaje claro en
///   vez de dejar que SQLite rechace un `NULL` más abajo.
/// - `radius` está **siempre** en el nivel superior (nunca hace falta el
///   fallback anidado a `statistics.radius`) -- pero a diferencia de
///   `mapStars.radius` (columna `INTEGER`), `mapPlanets.radius` es
///   `REAL`, así que se lee con [`optional_f64_with_nested_fallback`],
///   no la variante `i64`.
/// - `fragmented` **nunca** aparece, ni en el nivel superior ni anidado
///   (0 de 68407) -- en la práctica esta columna siempre sale `NULL`.
/// - `locked`, en cambio, está **siempre** anidado bajo `statistics`
///   (nunca en el nivel superior) -- lo opuesto a `radius`. Acá sí hace
///   falta el fallback para no perder el dato.
pub fn parse_planets(
    connection: &Connection,
    sde_directory: &Path,
    state: &SystemScopeState,
) -> Result<usize, BuilderError> {
    let mut insert_planet = connection.prepare(
        "INSERT INTO mapPlanets (planetId, solarSystemId, planetaryIndex, fragmented, radius, \
         locked, typeId, positionX, positionY, positionZ) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapPlanets")? {
        let record = record?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        if !state.systems_in_scope.contains(&solar_system_id) {
            continue;
        }

        let id = required_i64(&record, "_key")?;
        let planet_index = required_i64(&record, "celestialIndex")?;
        let fragmented = optional_bool_with_nested_fallback(&record, "fragmented", "statistics");
        let radius = optional_f64_with_nested_fallback(&record, "radius", "statistics");
        let locked = optional_bool_with_nested_fallback(&record, "locked", "statistics");
        let type_id = required_i64(&record, "typeID")?;
        let (pos_x, pos_y, pos_z) = required_position(&record)?;

        insert_planet.execute(rusqlite::params![
            id,
            solar_system_id,
            planet_index,
            fragmented,
            radius,
            locked,
            type_id,
            pos_x,
            pos_y,
            pos_z,
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// mapMoons
// ---------------------------------------------------------------------

/// Puebla `mapMoons` desde `<sde_directory>/mapMoons.jsonl`, filtrando
/// por `state.systems_in_scope` (poblado por [`parse_solar_systems`]).
/// Requiere que `mapSolarSystems` ya esté poblada (FK). Devuelve la
/// cantidad de filas insertadas. Equivalente a `_parse_moons()` en
/// Python.
///
/// # Nota: sin verificación contra datos reales
///
/// A diferencia de `mapStars`/`mapPlanets` (fases 6 y 7), acá **no** tuve
/// una muestra real de `mapMoons.jsonl` para verificar campo por campo
/// (el archivo pesa más de 200 MiB) -- este puerto se basa únicamente en
/// el código Python. Vale la pena aclarar que `mapMoons` es, según el
/// propio docstring de `_parse_moons()` en Python, la ENTIDAD DE
/// REFERENCIA cuyo shape sí está confirmado (`_key`, `attributes`,
/// `celestialIndex`, `npcStationIDs`, `orbitID`, `orbitIndex`,
/// `position`, `radius`, `solarSystemID`, `statistics`, `typeID`,
/// `uniqueName`) -- es la que se usó de base para *inferir sin verificar
/// independientemente* el shape de `mapStars`/`mapPlanets` en las dos
/// fases anteriores. Aun así, "confirmado" en ese docstring parece
/// referirse a los NOMBRES de los campos, no necesariamente a que estén
/// SIEMPRE presentes en todo registro -- ver la nota sobre `moonIndex`
/// más abajo.
///
/// `moonIndex` (`orbitIndex` en el JSON) se trata como requerido
/// ([`required_i64`]) aunque Python lo lee opcional
/// (`moon.get('orbitIndex')`) -- mismo criterio de siempre para columnas
/// `NOT NULL` (`mapMoons.moonIndex` lo es): si el campo genuinamente
/// faltara alguna vez, Python fallaría igual al insertar (constraint
/// violation), así que tratarlo como requerido acá no cambia el
/// resultado final (falla en ambos casos), solo da un mensaje más claro
/// y más temprano. La diferencia con `planetaryIndex` en la fase
/// anterior es que ahí sí pude confirmar con 68407 registros reales que
/// el campo nunca falta; acá es una inferencia a partir de ese mismo
/// patrón (y de la restricción `NOT NULL` del schema), no un hecho
/// verificado para `mapMoons` en particular.
///
/// `typeId` también se trata como requerido ([`required_i64`]),
/// coincidiendo con el acceso `moon['typeID']` (bracket) de Python -- a
/// pesar de que la columna en sí es nullable en el schema (`typeId
/// INTEGER REFERENCES invTypes(typeId)`, sin `NOT NULL`). Acá no hay
/// desviación de Python: el docstring confirma `typeID` como campo
/// presente en `mapMoons`, así que exigirlo es fidelidad al
/// comportamiento real de Python, no un endurecimiento propio.
pub fn parse_moons(
    connection: &Connection,
    sde_directory: &Path,
    state: &SystemScopeState,
) -> Result<usize, BuilderError> {
    let mut insert_moon = connection.prepare(
        "INSERT INTO mapMoons (moonId, solarSystemId, moonIndex, planetId, typeId, radius, \
         positionX, positionY, positionZ) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;

    let mut count = 0usize;
    for record in iter_jsonl_records(sde_directory, "mapMoons")? {
        let record = record?;
        let solar_system_id = required_i64(&record, "solarSystemID")?;
        if !state.systems_in_scope.contains(&solar_system_id) {
            continue;
        }

        let id = required_i64(&record, "_key")?;
        let moon_index = required_i64(&record, "orbitIndex")?;
        let planet_id = optional_i64(&record, "orbitID");
        let type_id = required_i64(&record, "typeID")?;
        let radius = optional_i64_with_nested_fallback(&record, "radius", "statistics");
        let (pos_x, pos_y, pos_z) = required_position(&record)?;

        insert_moon.execute(rusqlite::params![
            id,
            solar_system_id,
            moon_index,
            planet_id,
            type_id,
            radius,
            pos_x,
            pos_y,
            pos_z,
        ])?;
        count += 1;
    }
    Ok(count)
}

// ---------------------------------------------------------------------
// Orquestador
// ---------------------------------------------------------------------

/// Cantidad de filas insertadas por cada fase de [`parse_data`].
///
/// `star_types` cuenta las filas de `typeStar` (no una fase propia:
/// las genera [`parse_types`] al detectar tipos del grupo "Sun").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseSummary {
    pub categories: usize,
    pub groups: usize,
    pub types: usize,
    pub races: usize,
    pub npc_corporations: usize,
    pub factions: usize,
    pub star_types: usize,
    pub regions: usize,
    pub constellations: usize,
    pub solar_systems: usize,
    /// `0` tanto si no había gates que importar como si
    /// `config.with_gates` estaba en `false` (en ese caso, la fase ni
    /// siquiera se corre) -- no se distingue entre ambos casos.
    pub stargates: usize,
    pub stars: usize,
    pub planets: usize,
    /// `0` tanto si no había lunas que importar como si
    /// `config.with_moons` estaba en `false` -- no se distingue entre
    /// ambos casos, mismo criterio que `stargates`.
    pub moons: usize,
}

/// Corre el pipeline de parseo completo sobre `sde_directory`, en el mismo
/// orden de dependencias que `parse_data()` en Python. Equivalente a ese
/// método, salvo por el alcance actual (ver más abajo).
///
/// A diferencia de las funciones `parse_*` individuales -- que
/// autocommitean cada `INSERT` por separado, ver "Transacciones" en el
/// docstring del módulo --, esta función SÍ envuelve todo el pipeline en
/// una única transacción explícita (`Connection::transaction()`), igual
/// que Python, que no hace `commit()` hasta `SdeParser.close()`, al final
/// de todo. Si cualquier fase falla, se hace *rollback* de TODO lo
/// insertado hasta ese punto -- nada queda persistido a medias --, porque
/// el `Transaction` de rusqlite hace rollback automático en su `Drop` si
/// nunca se llamó a `.commit()`, y el operador `?` de cada llamada de
/// abajo dispara justamente ese `Drop` temprano al propagar el error.
///
/// Requiere `&mut Connection` (no `&Connection` como las funciones
/// individuales) porque `Connection::transaction()` lo exige.
///
/// ## Alcance actual
///
/// Hoy en día cubre las 13 funciones ya portadas (fase 1 a fase 8):
/// categorías, grupos, tipos (+ `typeStar`), razas, corporaciones NPC,
/// facciones (+ `factionRace`), regiones, constelaciones, sistemas
/// solares, stargates (condicional a `config.with_gates`), estrellas,
/// planetas y lunas (condicional a `config.with_moons`). Solo queda
/// `_parse_connections()`/`parse_connections()` en Python para llegar a
/// paridad completa con `parse_data()`.
pub fn parse_data(
    connection: &mut Connection,
    sde_directory: &Path,
    config: &ParserConfig,
) -> Result<ParseSummary, BuilderError> {
    let tx = connection.transaction()?;

    let categories = parse_categories(&tx, sde_directory, config)?;
    let mut state = StarTypeState::default();
    let groups = parse_groups(&tx, sde_directory, config, &mut state)?;
    let types = parse_types(&tx, sde_directory, config, &mut state)?;
    let races = parse_races(&tx, sde_directory, config)?;
    let npc_corporations = parse_npc_corporations(&tx, sde_directory, config)?;
    let factions = parse_factions(&tx, sde_directory, config)?;
    let regions = parse_regions(&tx, sde_directory, config)?;
    let constellations = parse_constellations(&tx, sde_directory, config)?;
    let mut scope = SystemScopeState::default();
    let solar_systems = parse_solar_systems(&tx, sde_directory, config, &mut scope)?;
    let stargates = if config.with_gates {
        parse_stargates(&tx, sde_directory, &scope)?
    } else {
        0
    };
    let stars = parse_stars(&tx, sde_directory, &scope, &state)?;
    let planets = parse_planets(&tx, sde_directory, &scope)?;
    let moons = if config.with_moons {
        parse_moons(&tx, sde_directory, &scope)?
    } else {
        0
    };

    tx.commit()?;

    Ok(ParseSummary {
        categories,
        groups,
        types,
        races,
        npc_corporations,
        factions,
        star_types: state.star_type_ids.len(),
        regions,
        constellations,
        solar_systems,
        stargates,
        stars,
        planets,
        moons,
    })
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
            ..Default::default()
        };
        let record: Value = serde_json::from_str(r#"{"name": {"en": "Jita", "de": "Jita"}}"#).unwrap();
        // "fr" no está presente -> cae a "en".
        assert_eq!(localized(&record, "name", &config), Some("Jita"));
    }

    #[test]
    fn localized_uses_requested_language_when_present() {
        let config = ParserConfig {
            language: "de".to_string(),
            ..Default::default()
        };
        let record: Value =
            serde_json::from_str(r#"{"name": {"en": "Jita", "de": "Jita (de)"}}"#).unwrap();
        assert_eq!(localized(&record, "name", &config), Some("Jita (de)"));
    }

    #[test]
    fn isometric_projection_2d_matches_python_reference_values() {
        // Valores de referencia calculados ejecutando
        // calculate_isometric_projection() de sde_parser.py directamente
        // con x=100.0, y=200.0, z=300.0 para cada projected_axis (0/1/2),
        // tomando de la tupla de 3 los dos componentes no forzados a 0.0.
        let (x, y, z) = (100.0, 200.0, 300.0);

        assert_eq!(
            isometric_projection_2d(x, y, z, ProjectedAxis::X),
            (100.0, 450.0)
        );
        assert_eq!(
            isometric_projection_2d(x, y, z, ProjectedAxis::Y),
            (-100.0, 450.0)
        );
        assert_eq!(
            isometric_projection_2d(x, y, z, ProjectedAxis::Z),
            (-200.0, 400.0)
        );
    }

    #[test]
    fn parser_config_default_uses_y_axis_and_does_not_force_isometric() {
        // Coincide con los defaults reales de SdeConfig en Python:
        // projection_algorithm='isometric', projected_axis=1 (Y) -- pero
        // aquí el "forzado" está apagado por default, ya que el
        // comportamiento normal es confiar en el position2D que ya trae
        // CCP cuando está presente.
        let config = ParserConfig::default();
        assert!(!config.force_isometric_position_2d);
        assert_eq!(config.isometric_projected_axis, ProjectedAxis::Y);
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

    #[test]
    fn parse_regions_inserts_rows_with_default_max_proj() {
        let dir = TempSdeDir::new(
            "regions",
            &[(
                "mapRegions.jsonl",
                "{\"_key\": 10000002, \"name\": {\"en\": \"The Forge\"}, \"nebulaID\": 5, \
                 \"position\": {\"x\": 100.0, \"y\": 200.0, \"z\": 300.0}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let count = parse_regions(&connection, &dir.path, &config).unwrap();
        assert_eq!(count, 1);

        let (name, faction_id, cx, cy, cz, nebula, wh_class, max_x, max_y): (
            String,
            Option<i64>,
            f64,
            f64,
            f64,
            i64,
            Option<i64>,
            f64,
            f64,
        ) = connection
            .query_row(
                "SELECT regionName, factionId, centerX, centerY, centerZ, nebula, \
                 wormholeClassId, maxProjX, maxProjY FROM mapRegions WHERE regionId = 10000002",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(name, "The Forge");
        assert_eq!(faction_id, None);
        assert_eq!((cx, cy, cz), (100.0, 200.0, 300.0));
        assert_eq!(nebula, 5);
        assert_eq!(wh_class, None);
        // maxProjX/maxProjY no se insertan explícitamente -- deben salir
        // del DEFAULT(0.0) del DDL.
        assert_eq!((max_x, max_y), (0.0, 0.0));
    }

    #[test]
    fn parse_regions_missing_nebula_errors() {
        // mapRegions.nebula es INTEGER NOT NULL; Python lo lee opcional
        // (`region.get('nebulaID')`) pero fallaría igual al insertar si
        // faltara -- ver "Desviaciones conocidas" en el docstring del
        // módulo.
        let dir = TempSdeDir::new(
            "regions_missing_nebula",
            &[(
                "mapRegions.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_regions(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_regions_missing_position_errors() {
        let dir = TempSdeDir::new(
            "regions_missing_position",
            &[(
                "mapRegions.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"nebulaID\": 0}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_regions(&connection, &dir.path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn parse_constellations_falls_back_to_key_when_constellation_id_absent() {
        let dir = TempSdeDir::new(
            "constellations_fallback",
            &[(
                "mapConstellations.jsonl",
                "{\"_key\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \"regionID\": 10000002, \
                 \"position\": {\"x\": 110.0, \"y\": 210.0, \"z\": 310.0}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        // mapRegions(10000002) lo exige la FK de mapConstellations.regionId.
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();

        let count = parse_constellations(&connection, &dir.path, &config).unwrap();
        assert_eq!(count, 1);

        let (id, name, region_id): (i64, String, i64) = connection
            .query_row(
                "SELECT constellationId, constellationName, regionId FROM mapConstellations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        // Sin `constellationID` en el registro, cae a `_key` (20000020).
        assert_eq!(id, 20000020);
        assert_eq!(name, "Kimotoro");
        assert_eq!(region_id, 10000002);
    }

    #[test]
    fn parse_constellations_prefers_constellation_id_when_present() {
        let dir = TempSdeDir::new(
            "constellations_prefer_id",
            &[(
                "mapConstellations.jsonl",
                "{\"_key\": 999, \"constellationID\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \
                 \"regionID\": 10000002, \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();

        parse_constellations(&connection, &dir.path, &config).unwrap();

        let id: i64 = connection
            .query_row("SELECT constellationId FROM mapConstellations", [], |row| {
                row.get(0)
            })
            .unwrap();
        // constellationID (20000020) gana sobre _key (999).
        assert_eq!(id, 20000020);
    }

    #[test]
    fn system_in_scope_kspace_gates_on_map_kspace() {
        let mut config = ParserConfig::default();
        assert!(system_in_scope(None, &config)); // default: map_kspace=true
        config.map_kspace = false;
        assert!(!system_in_scope(None, &config));
    }

    #[test]
    fn system_in_scope_wormhole_gates_on_any_of_three_flags() {
        let mut config = ParserConfig {
            map_wspace: false,
            map_abyssal: false,
            map_void: false,
            ..Default::default()
        };
        assert!(!system_in_scope(Some(5), &config));
        config.map_wspace = true;
        assert!(system_in_scope(Some(5), &config));
    }

    #[test]
    fn parse_solar_systems_inserts_kspace_system_with_ccp_position2d() {
        let dir = TempSdeDir::new(
            "solar_systems_kspace",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                 \"radius\": 999999999.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                 \"securityStatus\": 0.9459, \"securityClass\": \"B\", \"corridor\": false, \
                 \"fringe\": false, \"hub\": true, \"international\": true, \"regional\": true, \
                 \"luminosity\": 0.049, \"position2D\": {\"x\": 12.5, \"y\": -7.25}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let mut scope = SystemScopeState::default();

        let count = parse_solar_systems(&connection, &dir.path, &config, &mut scope).unwrap();
        assert_eq!(count, 1);
        assert!(scope.systems_in_scope.contains(&30000142));

        let (name, security, security_class, p2dx, p2dy): (String, f64, String, f64, f64) =
            connection
                .query_row(
                    "SELECT solarSystemName, security, securityClass, \
                     position2DX, position2DY FROM mapSolarSystems WHERE solarSystemId = 30000142",
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
        assert_eq!(name, "Jita");
        assert_eq!(security, 0.9459);
        assert_eq!(security_class, "B");
        // position2D sin forzar: el que ya trae el registro (12.5, -7.25),
        // NO el que calcularía isometric_projection_2d ((-300, -250), ver
        // el test de fuerza más abajo).
        assert_eq!((p2dx, p2dy), (12.5, -7.25));
    }

    #[test]
    fn parse_solar_systems_force_isometric_ignores_ccp_position2d() {
        let dir = TempSdeDir::new(
            "solar_systems_force_isometric",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                 \"radius\": 1.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                 \"securityStatus\": 0.9459, \"position2D\": {\"x\": 12.5, \"y\": -7.25}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        let config = ParserConfig {
            force_isometric_position_2d: true,
            ..Default::default()
        };
        let mut scope = SystemScopeState::default();

        parse_solar_systems(&connection, &dir.path, &config, &mut scope).unwrap();

        let (p2dx, p2dy): (f64, f64) = connection
            .query_row(
                "SELECT position2DX, position2DY FROM mapSolarSystems WHERE solarSystemId = 30000142",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // Forzado: debe ser el calculado (-300, -250), NO el (12.5, -7.25)
        // que trae el registro.
        assert_eq!((p2dx, p2dy), (-300.0, -250.0));
    }

    #[test]
    fn parse_solar_systems_excludes_out_of_scope_systems() {
        let dir = TempSdeDir::new(
            "solar_systems_scope",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"KSpace\"}, \"constellationID\": 20000020, \
                 \"radius\": 1.0, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"securityStatus\": 0.5}\n\
                 {\"_key\": 2, \"name\": {\"en\": \"WSpace\"}, \"constellationID\": 20000020, \
                 \"wormholeClassID\": 5, \"radius\": 1.0, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \"securityStatus\": -1.0}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        // Excluir k-space; w-space sigue habilitado por default.
        let config = ParserConfig {
            map_kspace: false,
            ..Default::default()
        };
        let mut scope = SystemScopeState::default();

        let count = parse_solar_systems(&connection, &dir.path, &config, &mut scope).unwrap();
        assert_eq!(count, 1);
        assert!(!scope.systems_in_scope.contains(&1));
        assert!(scope.systems_in_scope.contains(&2));

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSolarSystems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 1);
    }

    #[test]
    fn parse_solar_systems_missing_radius_errors() {
        let dir = TempSdeDir::new(
            "solar_systems_missing_radius",
            &[(
                "mapSolarSystems.jsonl",
                "{\"_key\": 1, \"name\": {\"en\": \"Test\"}, \"constellationID\": 20000020, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \"securityStatus\": 0.5}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let mut scope = SystemScopeState::default();

        let result = parse_solar_systems(&connection, &dir.path, &config, &mut scope);
        assert!(result.is_err());
    }

    /// Prerrequisitos de FK comunes a los tests de `parse_stargates`: dos
    /// sistemas solares (30000001, 30000002) en la misma constelación, y
    /// el tipo de item (16, "Stargate") que referencia `mapSystemGates.typeId`.
    fn insert_stargate_prerequisites(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (1, 'Celestial', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) \
                 VALUES (1, 1, 'Stargate Group', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (16, 1, 'Stargate', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        for (id, name) in [(30000001, "A"), (30000002, "B")] {
            connection
                .execute(
                    "INSERT INTO mapSolarSystems \
                     (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                     VALUES (?1, ?2, 20000020, 1.0, 0, 0, 0, 0.5)",
                    rusqlite::params![id, name],
                )
                .unwrap();
        }
    }

    /// Fixture de dos stargates que se referencian mutuamente: el gate
    /// 50000001 (en el sistema 30000001) apunta al 50000002 (en el
    /// 30000002), y viceversa -- el caso típico en datos reales del SDE.
    const MUTUAL_STARGATES_JSONL: &str =
        "{\"_key\": 50000001, \"solarSystemID\": 30000001, \"typeID\": 16, \
         \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
         \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30000002}}\n\
         {\"_key\": 50000002, \"solarSystemID\": 30000002, \"typeID\": 16, \
         \"position\": {\"x\": 4.0, \"y\": 5.0, \"z\": 6.0}, \
         \"destination\": {\"stargateID\": 50000001, \"solarSystemID\": 30000001}}\n";

    #[test]
    fn parse_stargates_without_transaction_fails_on_mutual_reference() {
        // Documenta el comportamiento descrito en el docstring de
        // parse_stargates: sin una transacción explícita, SQLite opera en
        // modo autocommit (cada INSERT es su propia transacción
        // implícita), así que la FK DEFERRABLE de destinationGateId igual
        // se valida de inmediato -- y el primer gate del par
        // necesariamente referencia a uno que todavía no existe.
        let dir = TempSdeDir::new("stargates_no_tx", &[("mapStargates.jsonl", MUTUAL_STARGATES_JSONL)]);
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let result = parse_stargates(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_stargates_within_transaction_inserts_mutual_reference() {
        let dir = TempSdeDir::new("stargates_tx", &[("mapStargates.jsonl", MUTUAL_STARGATES_JSONL)]);
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let tx = connection.transaction().unwrap();
        let count = parse_stargates(&tx, &dir.path, &scope).unwrap();
        assert_eq!(count, 2);
        tx.commit().unwrap();

        let (dest_gate, dest_system): (i64, i64) = connection
            .query_row(
                "SELECT destinationGateId, destinationSystemId FROM mapSystemGates \
                 WHERE systemGateId = 50000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(dest_gate, 50000002);
        assert_eq!(dest_system, 30000002);
    }

    #[test]
    fn parse_stargates_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "stargates_scope",
            &[(
                "mapStargates.jsonl",
                "{\"_key\": 50000003, \"solarSystemID\": 30000003, \"typeID\": 16, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"destination\": {\"stargateID\": 50000004, \"solarSystemID\": 30000001}}\n",
            )],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        // 30000003 NO está en el scope (a diferencia de 30000001/30000002).
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        scope.systems_in_scope.insert(30000002);

        let tx = connection.transaction().unwrap();
        let count = parse_stargates(&tx, &dir.path, &scope).unwrap();
        tx.commit().unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSystemGates", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_stargates_missing_type_id_errors() {
        let dir = TempSdeDir::new(
            "stargates_missing_type",
            &[(
                "mapStargates.jsonl",
                "{\"_key\": 50000001, \"solarSystemID\": 30000001, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30000002}}\n",
            )],
        );
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        insert_stargate_prerequisites(&connection);
        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);

        let result = parse_stargates(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    /// Setup común de los tests de `parse_stars`: crea el schema, un
    /// tipo de estrella detectado ("Sun" > "Yellow G5 (ffcc00)") vía
    /// `parse_groups`/`parse_types` directamente contra fixtures propios
    /// (para obtener un `StarTypeState` real, no simulado a mano), y un
    /// sistema solar en scope. Devuelve `(connection, star_state, scope)`.
    fn setup_for_parse_stars(dir_prefix: &str) -> (Connection, StarTypeState, SystemScopeState) {
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();

        let types_dir = TempSdeDir::new(
            dir_prefix,
            &[
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \
                     \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n",
                ),
            ],
        );
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (6, 'Celestial', 1)",
                [],
            )
            .unwrap();
        let config = ParserConfig::default();
        let mut star_state = StarTypeState::default();
        parse_groups(&connection, &types_dir.path, &config, &mut star_state).unwrap();
        parse_types(&connection, &types_dir.path, &config, &mut star_state).unwrap();

        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30000001, 'A', 20000020, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();

        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);

        (connection, star_state, scope)
    }

    #[test]
    fn parse_stars_inserts_row_using_real_sde_shape() {
        // Registro con el shape confirmado contra una muestra real de
        // mapStars.jsonl (agosto 2026): radius entero en el nivel
        // superior, statistics presente, sin locked en ningún lado.
        let dir = TempSdeDir::new(
            "stars_real_shape",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"radius\": 63350000, \"solarSystemID\": 30000001, \
                 \"statistics\": {\"age\": 4.5e17, \"life\": 6.9e17, \"luminosity\": 0.01575, \
                 \"spectralClass\": \"K2 V\", \"temperature\": 4567.0}, \"typeID\": 3000}\n",
            )],
        );
        let (connection, star_state, scope) = setup_for_parse_stars("stars_setup_real");

        let count = parse_stars(&connection, &dir.path, &scope, &star_state).unwrap();
        assert_eq!(count, 1);

        let (solar_system_id, locked, radius): (i64, Option<i64>, i64) = connection
            .query_row(
                "SELECT solarSystemId, locked, radius FROM mapStars WHERE starId = 40000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(solar_system_id, 30000001);
        assert_eq!(locked, None);
        assert_eq!(radius, 63350000);
    }

    #[test]
    fn parse_stars_locked_falls_back_to_nested_statistics() {
        // Sintético -- la SDE real nunca trae `locked` (ni en el nivel
        // superior ni en `statistics`), pero el fallback se porta igual
        // desde Python por si otra versión del SDE sí lo trae.
        let dir = TempSdeDir::new(
            "stars_locked_fallback",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"solarSystemID\": 30000001, \"typeID\": 3000, \
                 \"statistics\": {\"locked\": true}}\n",
            )],
        );
        let (connection, star_state, scope) = setup_for_parse_stars("stars_setup_fallback");

        parse_stars(&connection, &dir.path, &scope, &star_state).unwrap();

        let locked: Option<i64> = connection
            .query_row("SELECT locked FROM mapStars WHERE starId = 40000001", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(locked, Some(1));
    }

    #[test]
    fn parse_stars_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "stars_scope",
            &[(
                "mapStars.jsonl",
                "{\"_key\": 40000001, \"radius\": 1, \"solarSystemID\": 30000099, \"typeID\": 3000}\n",
            )],
        );
        let (connection, star_state, scope) = setup_for_parse_stars("stars_setup_scope");
        // 30000099 no está en el scope (solo 30000001 lo está).

        let count = parse_stars(&connection, &dir.path, &scope, &star_state).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapStars", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_stars_unknown_star_type_errors() {
        let dir = TempSdeDir::new(
            "stars_unknown_type",
            &[(
                "mapStars.jsonl",
                // typeID 9999 nunca fue detectado como tipo de estrella
                // por parse_types() en este fixture.
                "{\"_key\": 40000001, \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 9999}\n",
            )],
        );
        let (connection, star_state, scope) = setup_for_parse_stars("stars_setup_unknown");

        let result = parse_stars(&connection, &dir.path, &scope, &star_state);
        assert!(result.is_err());
    }

    /// Setup común de los tests de `parse_planets`: schema, un `invTypes`
    /// mínimo para satisfacer la FK de `typeId`, y un sistema solar en
    /// scope. Devuelve `(connection, scope)`.
    fn setup_for_parse_planets() -> (Connection, SystemScopeState) {
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (1, 'Celestial', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) \
                 VALUES (1, 1, 'Planet', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (11, 1, 'Planet (Barren)', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30000001, 'A', 20000020, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();

        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        (connection, scope)
    }

    #[test]
    fn parse_planets_inserts_row_using_real_sde_shape() {
        // Registro real de mapPlanets.jsonl (agosto 2026, EVE Online):
        // celestialIndex/position/typeID/solarSystemID siempre presentes;
        // radius en el nivel superior; locked SIEMPRE anidado bajo
        // statistics (nunca en el nivel superior); fragmented ausente.
        let dir = TempSdeDir::new(
            "planets_real_shape",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"position\": {\"x\": 161891117336.0, \"y\": 21288951986.0, \"z\": -73529712226.0}, \
                 \"radius\": 5060000, \"solarSystemID\": 30000001, \
                 \"statistics\": {\"locked\": false}, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_planets();

        let count = parse_planets(&connection, &dir.path, &scope).unwrap();
        assert_eq!(count, 1);

        let (planetary_index, fragmented, radius, locked, type_id): (
            i64,
            Option<i64>,
            f64,
            i64,
            i64,
        ) = connection
            .query_row(
                "SELECT planetaryIndex, fragmented, radius, locked, typeId \
                 FROM mapPlanets WHERE planetId = 40000002",
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
        assert_eq!(planetary_index, 1);
        assert_eq!(fragmented, None);
        assert_eq!(radius, 5060000.0);
        assert_eq!(locked, 0);
        assert_eq!(type_id, 11);
    }

    #[test]
    fn parse_planets_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "planets_scope",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"radius\": 1, \"solarSystemID\": 30000099, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_planets();
        // 30000099 no está en el scope (solo 30000001 lo está).

        let count = parse_planets(&connection, &dir.path, &scope).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapPlanets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_planets_missing_celestial_index_errors() {
        let dir = TempSdeDir::new(
            "planets_missing_index",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                 \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_planets();

        let result = parse_planets(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_planets_missing_position_errors() {
        let dir = TempSdeDir::new(
            "planets_missing_position",
            &[(
                "mapPlanets.jsonl",
                "{\"_key\": 40000002, \"celestialIndex\": 1, \
                 \"radius\": 1, \"solarSystemID\": 30000001, \"typeID\": 11}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_planets();

        let result = parse_planets(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    /// Setup común de los tests de `parse_moons`: schema, un `invTypes`
    /// para el planeta y otro para la luna, un sistema solar y un
    /// planeta en scope (para poder probar `planetId` con un valor real
    /// además de `NULL`). Devuelve `(connection, scope)`.
    fn setup_for_parse_moons() -> (Connection, SystemScopeState) {
        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (1, 'Celestial', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) \
                 VALUES (1, 1, 'Celestial Group', 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (11, 1, 'Planet (Barren)', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) \
                 VALUES (12, 1, 'Moon', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions \
                 (regionId, regionName, factionId, centerX, centerY, centerZ, nebula, wormholeClassId) \
                 VALUES (10000002, 'The Forge', NULL, 0, 0, 0, 5, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000020, 'Kimotoro', 10000002, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30000001, 'A', 20000020, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapPlanets \
                 (planetId, solarSystemId, planetaryIndex, typeId, positionX, positionY, positionZ) \
                 VALUES (40000002, 30000001, 1, 11, 0, 0, 0)",
                [],
            )
            .unwrap();

        let mut scope = SystemScopeState::default();
        scope.systems_in_scope.insert(30000001);
        (connection, scope)
    }

    #[test]
    fn parse_moons_inserts_row_with_planet_reference() {
        let dir = TempSdeDir::new(
            "moons_with_planet",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"orbitIndex\": 1, \
                 \"orbitID\": 40000002, \"typeID\": 12, \"radius\": 100000, \
                 \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();

        let count = parse_moons(&connection, &dir.path, &scope).unwrap();
        assert_eq!(count, 1);

        let (moon_index, planet_id, type_id, radius): (i64, Option<i64>, i64, Option<i64>) =
            connection
                .query_row(
                    "SELECT moonIndex, planetId, typeId, radius FROM mapMoons WHERE moonId = 40000004",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
        assert_eq!(moon_index, 1);
        assert_eq!(planet_id, Some(40000002));
        assert_eq!(type_id, 12);
        assert_eq!(radius, Some(100000));
    }

    #[test]
    fn parse_moons_without_orbit_id_leaves_planet_id_null() {
        // orbitID (planetId) es opcional -- tanto en Python
        // (`moon.get('orbitID')`) como en el schema (columna nullable).
        let dir = TempSdeDir::new(
            "moons_no_planet",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000005, \"solarSystemID\": 30000001, \"orbitIndex\": 2, \
                 \"typeID\": 12, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();

        parse_moons(&connection, &dir.path, &scope).unwrap();

        let planet_id: Option<i64> = connection
            .query_row(
                "SELECT planetId FROM mapMoons WHERE moonId = 40000005",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(planet_id, None);
    }

    #[test]
    fn parse_moons_skips_systems_outside_scope() {
        let dir = TempSdeDir::new(
            "moons_scope",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000099, \"orbitIndex\": 1, \
                 \"typeID\": 12, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();
        // 30000099 no está en el scope (solo 30000001 lo está).

        let count = parse_moons(&connection, &dir.path, &scope).unwrap();
        assert_eq!(count, 0);

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapMoons", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn parse_moons_missing_orbit_index_errors() {
        let dir = TempSdeDir::new(
            "moons_missing_index",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"typeID\": 12, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();

        let result = parse_moons(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_moons_missing_type_id_errors() {
        let dir = TempSdeDir::new(
            "moons_missing_type",
            &[(
                "mapMoons.jsonl",
                "{\"_key\": 40000004, \"solarSystemID\": 30000001, \"orbitIndex\": 1, \
                 \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}}\n",
            )],
        );
        let (connection, scope) = setup_for_parse_moons();

        let result = parse_moons(&connection, &dir.path, &scope);
        assert!(result.is_err());
    }

    #[test]
    fn parse_data_happy_path_returns_summary_and_commits() {
        let dir = TempSdeDir::new(
            "parse_data_happy",
            &[
                (
                    "categories.jsonl",
                    "{\"_key\": 6, \"name\": {\"en\": \"Celestial\"}, \"published\": true}\n",
                ),
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n\
                     {\"_key\": 7, \"categoryID\": 6, \"name\": {\"en\": \"Frigate\"}, \"anchorable\": false}\n",
                ),
                ("races.jsonl", "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n"),
                (
                    "npcCorporations.jsonl",
                    "{\"_key\": 1000004, \"name\": {\"en\": \"CBD Corporation\"}, \
                     \"tickerName\": \"CBD\", \"deleted\": false, \"iconID\": 500, \"raceID\": 1}\n",
                ),
                (
                    "factions.jsonl",
                    "{\"_key\": 500001, \"name\": {\"en\": \"Caldari State\"}, \"iconID\": 600, \
                     \"sizeFactor\": 3.0, \"uniqueName\": true, \"corporationID\": 1000004, \
                     \"memberRaces\": [1]}\n",
                ),
                (
                    "mapRegions.jsonl",
                    "{\"_key\": 10000002, \"name\": {\"en\": \"The Forge\"}, \"nebulaID\": 5, \
                     \"position\": {\"x\": 100.0, \"y\": 200.0, \"z\": 300.0}}\n",
                ),
                (
                    "mapConstellations.jsonl",
                    "{\"_key\": 20000020, \"name\": {\"en\": \"Kimotoro\"}, \"regionID\": 10000002, \
                     \"position\": {\"x\": 110.0, \"y\": 210.0, \"z\": 310.0}}\n",
                ),
                (
                    "mapSolarSystems.jsonl",
                    "{\"_key\": 30000142, \"name\": {\"en\": \"Jita\"}, \"constellationID\": 20000020, \
                     \"radius\": 999999999.0, \"position\": {\"x\": -100.0, \"y\": 200.0, \"z\": -300.0}, \
                     \"securityStatus\": 0.9459, \"securityClass\": \"B\", \"corridor\": false, \
                     \"fringe\": false, \"hub\": true, \"international\": true, \"regional\": true, \
                     \"luminosity\": 0.049, \"position2D\": {\"x\": 12.5, \"y\": -7.25}}\n\
                     {\"_key\": 30002187, \"name\": {\"en\": \"Perimeter\"}, \"constellationID\": 20000020, \
                     \"radius\": 1.0, \"position\": {\"x\": 0.0, \"y\": 0.0, \"z\": 0.0}, \
                     \"securityStatus\": 0.9}\n",
                ),
                (
                    "mapStargates.jsonl",
                    "{\"_key\": 50000001, \"solarSystemID\": 30000142, \"typeID\": 16, \
                     \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}, \
                     \"destination\": {\"stargateID\": 50000002, \"solarSystemID\": 30002187}}\n\
                     {\"_key\": 50000002, \"solarSystemID\": 30002187, \"typeID\": 16, \
                     \"position\": {\"x\": 4.0, \"y\": 5.0, \"z\": 6.0}, \
                     \"destination\": {\"stargateID\": 50000001, \"solarSystemID\": 30000142}}\n",
                ),
                (
                    "mapStars.jsonl",
                    "{\"_key\": 40000001, \"radius\": 63350000, \"solarSystemID\": 30000142, \
                     \"statistics\": {\"age\": 4.5e17, \"life\": 6.9e17, \"luminosity\": 0.01575, \
                     \"spectralClass\": \"K2 V\", \"temperature\": 4567.0}, \"typeID\": 3000}\n",
                ),
                (
                    "mapPlanets.jsonl",
                    "{\"_key\": 40000002, \"celestialIndex\": 1, \
                     \"position\": {\"x\": 161891117336.0, \"y\": 21288951986.0, \"z\": -73529712226.0}, \
                     \"radius\": 5060000, \"solarSystemID\": 30000142, \
                     \"statistics\": {\"locked\": false}, \"typeID\": 11}\n",
                ),
                (
                    "mapMoons.jsonl",
                    "{\"_key\": 40000004, \"solarSystemID\": 30000142, \"orbitIndex\": 1, \
                     \"orbitID\": 40000002, \"typeID\": 12, \"radius\": 100000, \
                     \"position\": {\"x\": 1.0, \"y\": 2.0, \"z\": 3.0}}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \
                     \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n\
                     {\"_key\": 16, \"groupID\": 7, \"name\": {\"en\": \"Stargate\"}, \"published\": true}\n\
                     {\"_key\": 11, \"groupID\": 7, \"name\": {\"en\": \"Planet (Barren)\"}, \"published\": true}\n\
                     {\"_key\": 12, \"groupID\": 7, \"name\": {\"en\": \"Moon\"}, \"published\": true}\n",
                ),
            ],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let summary = parse_data(&mut connection, &dir.path, &config).unwrap();
        assert_eq!(
            summary,
            ParseSummary {
                categories: 1,
                groups: 2,
                types: 4,
                races: 1,
                npc_corporations: 1,
                factions: 1,
                star_types: 1,
                regions: 1,
                constellations: 1,
                solar_systems: 2,
                stargates: 2,
                stars: 1,
                planets: 1,
                moons: 1,
            }
        );

        let total_faction_race: i64 = connection
            .query_row("SELECT COUNT(*) FROM factionRace", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total_faction_race, 1);

        let (dest_gate, dest_system): (i64, i64) = connection
            .query_row(
                "SELECT destinationGateId, destinationSystemId FROM mapSystemGates \
                 WHERE systemGateId = 50000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(dest_gate, 50000002);
        assert_eq!(dest_system, 30002187);
    }

    #[test]
    fn parse_data_rolls_back_everything_on_failure() {
        let dir = TempSdeDir::new(
            "parse_data_rollback",
            &[
                (
                    "categories.jsonl",
                    "{\"_key\": 6, \"name\": {\"en\": \"Celestial\"}, \"published\": true}\n",
                ),
                (
                    "groups.jsonl",
                    "{\"_key\": 6, \"categoryID\": 6, \"name\": {\"en\": \"Sun\"}, \"anchorable\": false}\n",
                ),
                (
                    "types.jsonl",
                    "{\"_key\": 3000, \"groupID\": 6, \"name\": {\"en\": \"Yellow G5 (ffcc00)\"}, \
                     \"iconID\": 100, \"published\": true, \"volume\": 0.0}\n",
                ),
                ("races.jsonl", "{\"_key\": 1, \"name\": {\"en\": \"Caldari\"}}\n"),
                (
                    "npcCorporations.jsonl",
                    "{\"_key\": 1000004, \"name\": {\"en\": \"CBD Corporation\"}, \
                     \"tickerName\": \"CBD\", \"deleted\": false, \"iconID\": 500, \"raceID\": 1}\n",
                ),
                (
                    // sizeFactor falta a propósito: factions.sizeFactor es
                    // REAL NOT NULL, así que parse_factions() debe fallar.
                    "factions.jsonl",
                    "{\"_key\": 500001, \"name\": {\"en\": \"Caldari State\"}, \"iconID\": 600, \
                     \"uniqueName\": true, \"corporationID\": 1000004}\n",
                ),
            ],
        );
        let mut connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        let config = ParserConfig::default();

        let result = parse_data(&mut connection, &dir.path, &config);
        assert!(result.is_err());

        // Nada debe haber quedado persistido, ni siquiera las fases
        // anteriores a la que falló (categories/groups/types/races/
        // npcCorporations ya se habían insertado exitosamente antes de
        // que factions fallara).
        for table in [
            "invCategories",
            "invGroups",
            "invTypes",
            "races",
            "npcCorporations",
            "factions",
            "factionRace",
            "typeStar",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "la tabla {table} debería estar vacía tras el rollback");
        }
    }
}
