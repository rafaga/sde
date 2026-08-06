//! Datos comunitarios de dotlan: tablas/columnas que NO forman parte del
//! SDE oficial de CCP, sino de fuentes externas (los mapas SVG de dotlan,
//! y listas mantenidas a mano por la comunidad como los sistemas con
//! Observatorio Jove o el estado de la invasión Triglavian).
//!
//! A diferencia de `builder::schema` (el DDL `STRICT` estático que
//! reconstruye el SDE canónico), este módulo crea sus tablas/columnas en
//! **tiempo de ejecución** (`CREATE TABLE`/`ALTER TABLE`), condicionadas a
//! [`DotlanConfig`] -- decisión explícita: incorporar esto al schema
//! estático excedería el objetivo primario del crate (reconstruir el SDE
//! tal cual, no enriquecerlo). Una base construida con, por ejemplo,
//! `with_icebelts: false` simplemente no tiene la columna `iceBelt` en
//! absoluto -- no es que exista vacía.
//!
//! `mapAbstractSystems` es la única excepción incondicional (igual que en
//! Python, donde `create_abstract_map()` corre siempre, sin flag): la
//! consume `SdeManager::get_abstract_systems()`/`get_abstract_connections()`
//! en el lado de lectura, así que si nunca corrió este módulo (o
//! [`update_tables`]) contra la base, esas dos consultas fallarán con
//! "no such table".
//!
//! Equivalente a `ExternalParser`/`ExternalConfig` en el prototipo Python
//! (`external_parser.py`).
//!
//! ## Alcance actual
//!
//! Cubre `update_tables()` completo: [`create_abstract_map`],
//! [`create_icebelts`], [`setup_triglavian_status`],
//! [`setup_jove_observatories`], [`setup_special_anomalies`], y ahora
//! también [`extract_map_data`] (parseo de un SVG ya descargado,
//! equivalente a `_extract_map_data()` en Python). Falta el orquestador
//! que descarga los mapas región por región y reintenta en caso de error
//! (`process()`), que reusará [`super::http`]/[`super::manifest`] ya
//! portados -- queda para una fase siguiente.

use crate::builder::BuilderError;
use rusqlite::Connection;
use std::path::Path;

/// Espacio de nombres XML de SVG -- los mapas de dotlan usan
/// `{http://www.w3.org/2000/svg}rect`/`use` en su XPath original de
/// Python; `roxmltree` expresa lo mismo con la forma tupla
/// `(namespace, local_name)` de [`roxmltree::Node::has_tag_name`].
const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// Lista de sistemas con Observatorio Jove, uno por línea. Extraída
/// programáticamente (no transcrita a mano) de las tres listas
/// concatenadas en `create_jove_observatories()` del Python original --
/// 1032 nombres en la fuente, con 3 duplicados exactos (`Eygfe`,
/// `MJYW-3`, `Odinesyn`) que aparecían ya duplicados ahí mismo; acá se
/// deduplican (1029 nombres únicos, mismo resultado en la práctica: un
/// `UPDATE ... WHERE x IN (...)` no cambia de comportamiento por una
/// entrada repetida, así que no hay pérdida de fidelidad, solo menos
/// texto redundante).
const JOVE_OBSERVATORY_SYSTEMS: &str = include_str!("jove_observatories.txt");

/// Config para los datos comunitarios de dotlan -- equivalente a
/// `ExternalConfig` en Python, mismos defaults (incluyendo
/// `with_jove_observatories: true`, la única bandera que arranca en
/// `true` de las cuatro).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DotlanConfig {
    pub with_icebelts: bool,
    pub with_triglavian_status: bool,
    pub with_jove_observatories: bool,
    pub with_special_ore: bool,
}

impl Default for DotlanConfig {
    fn default() -> Self {
        Self {
            with_icebelts: false,
            with_triglavian_status: false,
            with_jove_observatories: true,
            with_special_ore: false,
        }
    }
}

/// Crea `mapAbstractSystems`, la tabla donde el parseo de los SVG de
/// dotlan (aún por portar) inserta las coordenadas 2D "abstractas" que
/// dotlan calcula para su propio layout de mapa -- no relacionadas con
/// `mapSolarSystems.position2DX/Y`, que vienen del SDE oficial. Se crea
/// siempre, sin condicionar a ningún flag de [`DotlanConfig`] -- mismo
/// comportamiento que `create_abstract_map()` en Python.
///
/// `x`/`y` son `REAL`, no `INTEGER` como en el DDL original de Python: la
/// tabla del Python original no es `STRICT` (SQLite clásico acepta un
/// valor fraccionario en una columna "INT" igual, por afinidad de tipo),
/// pero acá sí lo es -- y las coordenadas de un `<use x="..." y="...">`
/// SVG son casi con certeza fraccionarias. `REAL` además ya es lo que
/// espera `SdeManager::get_abstract_systems()` en el lado de lectura
/// (`row.get::<usize, f32>(...)`), y lo que ya usa el fixture de
/// `tests/manager.rs`.
pub fn create_abstract_map(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "CREATE TABLE mapAbstractSystems ( \
            solarSystemId INTEGER NOT NULL \
                REFERENCES mapSolarSystems(solarSystemId) \
                ON UPDATE CASCADE ON DELETE SET NULL, \
            regionId INTEGER NOT NULL \
                REFERENCES mapRegions(regionId) \
                ON UPDATE CASCADE ON DELETE SET NULL, \
            x REAL NOT NULL, y REAL NOT NULL, \
            CONSTRAINT pkey PRIMARY KEY (solarSystemId, regionId) ON CONFLICT FAIL \
        ) STRICT;",
    )?;
    Ok(())
}

/// Agrega la columna `iceBelt` (y su índice) a `mapSolarSystems` --
/// **solo la estructura**, no la puebla. El poblado real depende del
/// parseo de cada SVG regional (`<rect class="i" id="...">`), así que
/// vive junto a ese parseo (fase siguiente), no acá. Equivalente a
/// `create_icebelts()` en Python.
pub fn create_icebelts(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "ALTER TABLE mapSolarSystems ADD COLUMN iceBelt \
            INTEGER NOT NULL DEFAULT 0 CHECK (iceBelt IN (0,1)); \
         CREATE INDEX icebelts ON mapSolarSystems (solarSystemId, iceBelt);",
    )?;
    Ok(())
}

/// Crea `mapTriglavianStatus` (con sus 5 filas fijas), agrega
/// `mapSolarSystems.trigStatusID`, y marca los sistemas correspondientes
/// a cada uno de los 4 estados no-`None` -- estructura y datos juntos, en
/// una sola función, igual que `create_triglavian()` en Python (que
/// tampoco separa ambas cosas: la triglavian son 192 ids fijos escritos
/// en el propio código, no algo derivado de una fuente externa aparte
/// como el SVG).
///
/// # Desviación necesaria de Python: sin `DEFAULT 0` en la columna
///
/// Python declara la columna como
/// `trigStatusID INTEGER DEFAULT 0 REFERENCES mapTriglavianStatus(...)`.
/// Verificado contra sqlite3 real: esa combinación -- `DEFAULT` no-nulo
/// + `REFERENCES` -- **SQLite la rechaza** en un `ALTER TABLE ADD
/// COLUMN` (`Cannot add a REFERENCES column with non-NULL default
/// value`), sin importar si además se declara `NOT NULL` o no. Como
/// `with_triglavian_status` arranca en `false` en el propio Python, es
/// casi seguro que esta rama nunca se ejecutó de verdad contra una base
/// real -- el bug nunca se manifestó.
///
/// La columna acá queda sin `DEFAULT` explícito (nullable, `NULL`
/// implícito) -- SQLite sí permite esa combinación con `REFERENCES`.
/// `NULL` es equivalente semántico de `trigStatusID=0` ('None'): un
/// sistema sin marcar no tiene status especial en ningún caso. La FK
/// sigue activa y validándose con normalidad (verificado: un valor fuera
/// de las 5 filas de `mapTriglavianStatus` sigue siendo rechazado).
pub fn setup_triglavian_status(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "CREATE TABLE mapTriglavianStatus ( \
            trigStatusId INTEGER NOT NULL PRIMARY KEY, \
            trigStatusName TEXT NOT NULL \
         ) STRICT; \
         INSERT INTO mapTriglavianStatus (trigStatusId, trigStatusName) VALUES \
            (0, 'None'), \
            (1, 'Edencom Minor Victory'), \
            (2, 'Final Liminality'), \
            (3, 'Fortress'), \
            (4, 'Triglavian Minor Victory'); \
         ALTER TABLE mapSolarSystems ADD COLUMN trigStatusID \
            INTEGER REFERENCES mapTriglavianStatus(trigStatusID) \
            ON UPDATE CASCADE ON DELETE SET NULL; \
         CREATE INDEX trigStatus ON mapSolarSystems (solarSystemId, trigStatusID);",
    )?;

    for (status_id, ids) in [
        (1, EDENCOM_MINOR_VICTORY),
        (2, FINAL_LIMINALITY),
        (3, FORTRESS),
        (4, TRIGLAVIAN_MINOR_VICTORY),
    ] {
        let mut statement = connection.prepare(
            "UPDATE mapSolarSystems SET trigStatusID = ?1 WHERE solarSystemId = ?2",
        )?;
        for &solar_system_id in ids {
            statement.execute(rusqlite::params![status_id, solar_system_id])?;
        }
    }
    Ok(())
}

/// Agrega `mapSolarSystems.joveObservatory` (con su índice) y marca los
/// 1029 sistemas de [`JOVE_OBSERVATORY_SYSTEMS`]. Equivalente a
/// `create_jove_observatories()` en Python.
pub fn setup_jove_observatories(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "ALTER TABLE mapSolarSystems ADD COLUMN joveObservatory \
            INTEGER NOT NULL DEFAULT 0 CHECK (joveObservatory IN (0,1)); \
         CREATE INDEX joveSystems ON mapSolarSystems (solarSystemId, joveObservatory);",
    )?;

    let mut statement = connection
        .prepare("UPDATE mapSolarSystems SET joveObservatory = 1 WHERE solarSystemName = ?1")?;
    for name in JOVE_OBSERVATORY_SYSTEMS.lines() {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        statement.execute(rusqlite::params![name])?;
    }
    Ok(())
}

/// Agrega `mapSolarSystems.specialOreAnom` y marca los sistemas cuya
/// estrella es de tipo espectral "A0" (`typeStar.name`). Equivalente a
/// `create_special_anomalies()` en Python -- con `ts.name` explícito en
/// vez del `name` sin calificar del original (ambiguo solo en apariencia:
/// `mapStars` no tiene columna `name`, así que ya resolvía a
/// `typeStar.name` de todos modos; acá se deja explícito por claridad,
/// sin cambiar el resultado).
pub fn setup_special_anomalies(connection: &Connection) -> Result<(), BuilderError> {
    connection.execute_batch(
        "ALTER TABLE mapSolarSystems ADD COLUMN specialOreAnom \
            INTEGER NOT NULL DEFAULT 0 CHECK (specialOreAnom IN (0,1));",
    )?;
    connection.execute(
        "UPDATE mapSolarSystems SET specialOreAnom = 1 WHERE solarSystemId IN ( \
            SELECT m.solarSystemId FROM typeStar AS ts \
            INNER JOIN mapStars AS m ON (ts.starTypeId = m.starTypeId) \
            WHERE ts.name = ?1 \
         )",
        rusqlite::params!["A0"],
    )?;
    Ok(())
}

/// Crea (y puebla, donde corresponde) todas las tablas/columnas de datos
/// comunitarios habilitadas por `config`. `mapAbstractSystems` corre
/// siempre; el resto respeta cada flag de [`DotlanConfig`]. Equivalente a
/// `_update_tables()` en Python.
///
/// Nota: a diferencia de [`crate::builder::parser::parse_data`], esta
/// función NO envuelve las llamadas en una transacción explícita -- cada
/// `CREATE TABLE`/`ALTER TABLE` es una operación DDL independiente y no
/// hay ninguna FK circular entre ellas que dependa de verse todas juntas
/// (a diferencia del caso de `mapSystemGates.destinationGateId`
/// documentado en `parser::parse_stargates`). Si se agrega el flujo de
/// `mapAbstractSystems` con SVG en la fase siguiente, sí conviene
/// reconsiderar esto.
pub fn update_tables(connection: &Connection, config: &DotlanConfig) -> Result<(), BuilderError> {
    create_abstract_map(connection)?;
    if config.with_icebelts {
        create_icebelts(connection)?;
    }
    if config.with_triglavian_status {
        setup_triglavian_status(connection)?;
    }
    if config.with_jove_observatories {
        setup_jove_observatories(connection)?;
    }
    if config.with_special_ore {
        setup_special_anomalies(connection)?;
    }
    Ok(())
}

/// Parsea un mapa SVG de dotlan ya descargado (`map_path`), extrayendo
/// los ids de sistemas con icebelt y las coordenadas "abstractas" de cada
/// sistema para `mapAbstractSystems`. Equivalente a
/// `_extract_map_data()` en Python.
///
/// Devuelve `Ok(false)` -- no `Err` -- tanto si el archivo no existe como
/// si el XML no parsea: mismo comportamiento que Python, donde ambos
/// casos hacen que el orquestador (`process()`, aún por portar) reintente
/// la descarga en vez de abortar todo el build. Un `Err` genuino acá solo
/// puede venir de un error real de SQLite -- por ejemplo, si el nombre de
/// región derivado del archivo no matchea ninguna fila de `mapRegions`,
/// lo que viola el `NOT NULL` de `mapAbstractSystems.regionId` (mismo
/// comportamiento que el `INSERT` con subconsulta de Python: la
/// subconsulta `(SELECT regionId FROM mapRegions WHERE regionName=...)`
/// devuelve `NULL` si no hay match, y el `INSERT` falla).
///
/// El nombre de región se deriva del nombre de archivo, no se recibe
/// aparte: `The_Forge.svg` -> `"The Forge"` (guiones bajos a espacios) --
/// igual que Python, salvo que se usa [`Path::file_stem`] (quita todo
/// después del ÚLTIMO punto) en vez del `split('.')[0]` de Python (quita
/// todo después del PRIMER punto); para un nombre de archivo típico como
/// este, sin puntos de por medio, ambos dan el mismo resultado.
///
/// # Formato de los `id`: confirmado contra un mapa real de dotlan
///
/// Verificado contra `Derelik.svg` (un mapa regional real, agosto 2026):
/// los `<rect class="i">` usan el prefijo `ice` (11 de 12 rects con esa
/// clase traían `id="iceNNNNNNNN"`; el rect restante, sin `id` en
/// absoluto, resultó ser una entrada de leyenda/referencia visual, no un
/// sistema -- confirma que el guard `let Some(raw_id) = ... else {
/// continue }` es necesario, no defensivo de más). Los `<use>` (sistemas
/// abstractos) usan el prefijo `sys` -- los 125 `<use>` del archivo
/// traían `id`/`x`/`y` completos, sin faltantes. Ambos prefijos son de 3
/// caracteres, coincidiendo con el `tag_id[3::]` de Python -- el código
/// no depende del TEXTO del prefijo (nunca lo compara), solo descarta 3
/// bytes fijos, así que funciona igual para `ice`/`sys` o cualquier otro
/// prefijo de 3 caracteres ASCII que aparezca en otras regiones.
///
/// Si algún otro mapa regional trajera un formato distinto de todos
/// modos, esta función no rompe: cualquier `id`/`x`/`y` que no parsee
/// como número simplemente se omite (con una advertencia en stderr),
/// fila por fila, sin abortar el resto del parseo.
///
/// A diferencia de Python (que pasa el `id` como string tal cual,
/// confiando en la coerción de tipos clásica de SQLite -- su
/// `mapAbstractSystems` no es `STRICT`), acá los tres valores
/// (`solarSystemId`, `x`, `y`) se parsean explícitamente a `i64`/`f64`
/// antes de bindearlos: no es una preferencia de estilo, es un requisito
/// real de las tablas `STRICT` de este crate, que no aceptan una
/// coerción implícita de texto a número como sí hace SQLite clásico.
pub fn extract_map_data(
    connection: &Connection,
    map_path: &Path,
    config: &DotlanConfig,
) -> Result<bool, BuilderError> {
    if !map_path.exists() {
        eprintln!("dotlan: {} no existe, se omite el parseo", map_path.display());
        return Ok(false);
    }

    let content = std::fs::read_to_string(map_path)?;
    let doc = match roxmltree::Document::parse(&content) {
        Ok(doc) => doc,
        Err(err) => {
            eprintln!("dotlan: error parseando {} - {err}", map_path.display());
            return Ok(false);
        }
    };

    // --- icebelts: <rect class="i" id="..."> ---
    let mut icebelt_ids = Vec::new();
    for tag in doc
        .descendants()
        .filter(|n| n.has_tag_name((SVG_NS, "rect")) && n.attribute("class") == Some("i"))
    {
        let Some(raw_id) = tag.attribute("id") else {
            continue;
        };
        match raw_id.get(3..).and_then(|s| s.parse::<i64>().ok()) {
            Some(id) => icebelt_ids.push(id),
            None => eprintln!(
                "dotlan: id de icebelt '{raw_id}' inesperado en {}, se omite",
                map_path.display()
            ),
        }
    }
    if !icebelt_ids.is_empty() && config.with_icebelts {
        let mut statement =
            connection.prepare("UPDATE mapSolarSystems SET iceBelt = 1 WHERE solarSystemId = ?1")?;
        for id in &icebelt_ids {
            statement.execute(rusqlite::params![id])?;
        }
    }

    // --- mapAbstractSystems: <use id="..." x="..." y="..."> ---
    let region_name = map_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.replace('_', " "))
        .ok_or_else(|| {
            BuilderError::Data(format!(
                "no se pudo derivar el nombre de región de {}",
                map_path.display()
            ))
        })?;

    let mut insert_abstract = connection.prepare(
        "INSERT INTO mapAbstractSystems (solarSystemId, regionId, x, y) \
         VALUES (?1, (SELECT regionId FROM mapRegions WHERE regionName = ?2), ?3, ?4)",
    )?;
    for tag in doc.descendants().filter(|n| n.has_tag_name((SVG_NS, "use"))) {
        let (Some(raw_id), Some(raw_x), Some(raw_y)) =
            (tag.attribute("id"), tag.attribute("x"), tag.attribute("y"))
        else {
            continue;
        };
        let parsed = raw_id
            .get(3..)
            .and_then(|s| s.parse::<i64>().ok())
            .zip(raw_x.parse::<f64>().ok())
            .zip(raw_y.parse::<f64>().ok());
        let Some(((id, x), y)) = parsed else {
            eprintln!(
                "dotlan: <use id='{raw_id}' x='{raw_x}' y='{raw_y}'> inesperado en {}, se omite",
                map_path.display()
            );
            continue;
        };
        insert_abstract.execute(rusqlite::params![id, region_name, x, y])?;
    }

    Ok(true)
}

const EDENCOM_MINOR_VICTORY: &[i64] = &[
    30003088, 30003894, 30004302, 30005074, 30003570, 30003463, 30003788, 30002724, 30002999,
    30000102, 30003919, 30004978, 30004287, 30002051, 30003823, 30005267, 30003587, 30003904,
    30005209, 30005219, 30002755, 30003824, 30002239, 30003794, 30003927, 30000109, 30000060,
    30000160, 30004999, 30004295, 30004231, 30004284, 30003932, 30004254, 30002513, 30002048,
    30003090, 30003478, 30004289, 30003061, 30003078, 30003900, 30002644, 30003480, 30001696,
    30002772, 30005284, 30005222, 30005086, 30003918, 30003908, 30000012, 30003481, 30003460,
    30005213, 30005308, 30003058, 30005334, 30002506, 30003931, 30005255, 30004263, 30000062,
    30002241, 30003558, 30001376, 30004257, 30004108, 30000048, 30003482, 30005263, 30005066,
    30004268, 30005236, 30003829, 30005034, 30003074, 30003809, 30001718, 30004256, 30004301,
    30002397, 30003854, 30001660,
];

const FINAL_LIMINALITY: &[i64] = &[
    30002079, 30002652, 30002411, 30005005, 30000021, 30002797, 30031392, 30001413, 30000206,
    30040141, 30045328, 30002770, 30003504, 30002737, 30000192, 30000157, 30001372, 30002702,
    30003046, 30020141, 30045329, 30002225, 30001381, 30001445, 30010141, 30005029, 30003495,
];

const FORTRESS: &[i64] = &[
    30003539, 30003573, 30005251, 30004103, 30000118, 30004090, 30003548, 30000113, 30002386,
    30004973, 30002266, 30002530, 30004141, 30002253, 30003398, 30003490, 30003556, 30002385,
    30002704, 30005058, 30004305, 30003883, 30003397, 30004084, 30003574, 30002665, 30000188,
    30003514, 30005052, 30000004, 30002242, 30005252, 30002986, 30002700, 30003050, 30000005,
    30004250, 30003392, 30003515, 30004100, 30002662, 30045322, 30003885, 30004248, 30003541,
    30002651, 30004150, 30002251, 30005260, 30000105, 30004992, 30002243, 30003553,
];

const TRIGLAVIAN_MINOR_VICTORY: &[i64] = &[
    30045331, 30001400, 30004244, 30045345, 30001358, 30001401, 30045354, 30002557, 30002760,
    30002795, 30004981, 30001447, 30001390, 30003076, 30000163, 30003073, 30001391, 30002771,
    30005330, 30000205, 30003856, 30002645, 30045338, 30002575, 30001383, 30003464, 30000182,
    30001685,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Prerrequisitos comunes: schema completo + una región/constelación
    /// y dos sistemas solares (uno con id de la lista de Edencom, para
    /// probar el poblado de triglavian; el otro nombrado como el primer
    /// sistema real de `jove_observatories.txt`, para probar el
    /// matching exacto por nombre).
    fn setup(connection: &Connection) {
        crate::builder::schema::create_schema(connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions (regionId, regionName, nebula, centerX, centerY, centerZ) \
                 VALUES (10000002, 'The Forge', 5, 0, 0, 0)",
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
        // 30003088 es el primer id de EDENCOM_MINOR_VICTORY.
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30003088, 'Sys Edencom', 20000020, 1.0, 0, 0, 0, 0.5)",
                [],
            )
            .unwrap();
        // "0-4VQL" es la primera linea real de jove_observatories.txt.
        connection
            .execute(
                "INSERT INTO mapSolarSystems \
                 (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                 VALUES (30000001, '0-4VQL', 20000020, 1.0, 0, 0, 0, 0.9)",
                [],
            )
            .unwrap();
    }

    /// Escribe un SVG temporal con el nombre exacto `file_name` (para que
    /// `extract_map_data` derive el nombre de región correcto) dentro de
    /// un directorio temporal único, y devuelve su path. El directorio no
    /// se borra automáticamente -- son archivos de unos pocos bytes en
    /// `std::env::temp_dir()`, igual de descartable que el resto de los
    /// fixtures de este crate.
    fn write_temp_svg(test_name: &str, file_name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sde-dotlan-test-{test_name}-{}-{}",
            std::process::id(),
            file_name.len() // dispersor barato para no colisionar entre tests
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file_name);
        std::fs::write(&path, content).unwrap();
        path
    }

    const SAMPLE_SVG: &str = concat!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\">",
        "<rect class=\"i\" id=\"sys30003088\"/>",
        "<rect class=\"j\" id=\"sys30000001\"/>",
        "<use id=\"sys30000001\" x=\"12.5\" y=\"-7.25\"/>",
        "</svg>",
    );

    #[test]
    fn extract_map_data_returns_false_when_file_missing() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();

        let missing = std::env::temp_dir().join("sde-dotlan-test-no-existe.svg");
        let config = DotlanConfig::default();
        let result = extract_map_data(&connection, &missing, &config).unwrap();
        assert!(!result);
    }

    #[test]
    fn extract_map_data_returns_false_on_malformed_svg() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();

        let path = write_temp_svg(
            "malformed",
            "The_Forge.svg",
            "<svg><rect></svg>",
        );
        let config = DotlanConfig::default();
        let result = extract_map_data(&connection, &path, &config).unwrap();
        assert!(!result);
    }

    #[test]
    fn extract_map_data_inserts_abstract_systems_with_derived_region_name() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();

        let path = write_temp_svg("abstract_systems", "The_Forge.svg", SAMPLE_SVG);
        let config = DotlanConfig::default();
        let ok = extract_map_data(&connection, &path, &config).unwrap();
        assert!(ok);

        let (region_id, x, y): (i64, f64, f64) = connection
            .query_row(
                "SELECT regionId, x, y FROM mapAbstractSystems WHERE solarSystemId = 30000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        // "The_Forge.svg" -> region "The Forge" -> regionId 10000002 (del fixture).
        assert_eq!(region_id, 10000002);
        assert_eq!((x, y), (12.5, -7.25));
    }

    #[test]
    fn extract_map_data_updates_icebelt_only_when_enabled() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();
        create_icebelts(&connection).unwrap();

        let path = write_temp_svg("icebelt_disabled", "The_Forge.svg", SAMPLE_SVG);
        // with_icebelts=false (default): el rect se parsea pero NO se escribe.
        let config = DotlanConfig::default();
        extract_map_data(&connection, &path, &config).unwrap();
        let ice_belt: i64 = connection
            .query_row(
                "SELECT iceBelt FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_belt, 0, "with_icebelts=false no deberia escribir nada");

        let config_enabled = DotlanConfig { with_icebelts: true, ..config };
        extract_map_data(&connection, &path, &config_enabled).unwrap();
        let ice_belt: i64 = connection
            .query_row(
                "SELECT iceBelt FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_belt, 1, "with_icebelts=true si deberia marcarlo");
    }

    #[test]
    fn extract_map_data_skips_use_tags_missing_attributes() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        create_abstract_map(&connection).unwrap();

        let svg = concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\">",
            "<use id=\"sys30000001\" x=\"1.0\"/>", // falta y
            "<use x=\"1.0\" y=\"2.0\"/>",           // falta id
            "</svg>",
        );
        let path = write_temp_svg("incomplete_use", "The_Forge.svg", svg);
        let config = DotlanConfig::default();
        let ok = extract_map_data(&connection, &path, &config).unwrap();
        assert!(ok, "un <use> incompleto se omite, no aborta el parseo");

        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapAbstractSystems", [], |row| row.get(0))
            .unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn extract_map_data_handles_real_dotlan_excerpt() {
        // Extracto textual EXACTO de un mapa real de dotlan (Derelik.svg,
        // agosto 2026) -- no sintetizado a mano: el rect de leyenda sin
        // `id`, dos rects de icebelt reales (prefijo `ice`), y dos `<use>`
        // reales (prefijo `sys`), tal como aparecen en el archivo
        // original.
        let svg = concat!(
            "<svg version=\"1.1\" xmlns=\"http://www.w3.org/2000/svg\" ",
            "xmlns:xlink=\"http://www.w3.org/1999/xlink\">",
            "<rect x=\"872\" y=\"726\" rx=\"5.5\" ry=\"5.5\" width=\"15.4\" height=\"11\" class=\"i\" />",
            "<rect id=\"ice30000072\" x=\"1\" y=\"0.5\" rx=\"14\" ry=\"13\" width=\"56\" height=\"28\" class=\"i\" />",
            "<rect id=\"ice30000087\" x=\"1\" y=\"0.5\" rx=\"14\" ry=\"13\" width=\"56\" height=\"28\" class=\"i\" />",
            "<use id=\"sys30000071\" x=\"0\" y=\"500\" width=\"62.5\" height=\"30\" xlink:href=\"#def30000071\" />",
            "<use id=\"sys30000074\" x=\"0\" y=\"555\" width=\"62.5\" height=\"30\" xlink:href=\"#def30000074\" />",
            "</svg>",
        );

        let connection = Connection::open_in_memory().unwrap();
        crate::builder::schema::create_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapRegions (regionId, regionName, nebula, centerX, centerY, centerZ) \
                 VALUES (10000005, 'Derelik', 5, 0, 0, 0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapConstellations \
                 (constellationId, constellationName, regionId, centerX, centerY, centerZ) \
                 VALUES (20000005, 'Konora', 10000005, 0, 0, 0)",
                [],
            )
            .unwrap();
        for id in [30000071, 30000072, 30000074, 30000087] {
            connection
                .execute(
                    "INSERT INTO mapSolarSystems \
                     (solarSystemId, solarSystemName, constellationId, radius, centerX, centerY, centerZ, security) \
                     VALUES (?1, ?1, 20000005, 1.0, 0, 0, 0, 0.5)",
                    rusqlite::params![id],
                )
                .unwrap();
        }
        create_abstract_map(&connection).unwrap();
        create_icebelts(&connection).unwrap();

        let path = write_temp_svg("real_excerpt", "Derelik.svg", svg);
        let config = DotlanConfig { with_icebelts: true, ..DotlanConfig::default() };
        let ok = extract_map_data(&connection, &path, &config).unwrap();
        assert!(ok);

        // El rect de leyenda (sin id) no debe generar ningun UPDATE de mas
        // -- solo los dos con id real quedan marcados.
        let ice_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapSolarSystems WHERE iceBelt = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(ice_count, 2);

        let (region_id, x, y): (i64, f64, f64) = connection
            .query_row(
                "SELECT regionId, x, y FROM mapAbstractSystems WHERE solarSystemId = 30000071",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(region_id, 10000005);
        assert_eq!((x, y), (0.0, 500.0));
    }

    #[test]
    fn jove_observatory_list_has_no_duplicates_and_no_blank_lines() {
        let names: Vec<&str> = JOVE_OBSERVATORY_SYSTEMS.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(names.len(), 1029);
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), 1029, "no deberian quedar duplicados tras la deduplicacion");
    }

    #[test]
    fn create_abstract_map_accepts_fractional_coords() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        create_abstract_map(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO mapAbstractSystems (solarSystemId, regionId, x, y) \
                 VALUES (30000001, 10000002, 12.5, -7.25)",
                [],
            )
            .unwrap();

        let (x, y): (f64, f64) = connection
            .query_row(
                "SELECT x, y FROM mapAbstractSystems WHERE solarSystemId = 30000001",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((x, y), (12.5, -7.25));
    }

    #[test]
    fn create_icebelts_adds_column_defaulting_to_zero() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        create_icebelts(&connection).unwrap();
        let ice_belt: i64 = connection
            .query_row(
                "SELECT iceBelt FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_belt, 0);

        connection
            .execute(
                "UPDATE mapSolarSystems SET iceBelt = 1 WHERE solarSystemId = 30000001",
                [],
            )
            .unwrap();
        let ice_belt: i64 = connection
            .query_row(
                "SELECT iceBelt FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ice_belt, 1);
    }

    #[test]
    fn setup_triglavian_status_marks_known_systems_and_defaults_to_null() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        setup_triglavian_status(&connection).unwrap();

        let status_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM mapTriglavianStatus", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status_count, 5);

        // 30003088 esta en EDENCOM_MINOR_VICTORY -> trigStatusID=1.
        let marked: Option<i64> = connection
            .query_row(
                "SELECT trigStatusID FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, Some(1));

        // 30000001 no esta en ninguna lista -> NULL (no 0).
        let unmarked: Option<i64> = connection
            .query_row(
                "SELECT trigStatusID FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unmarked, None);
    }

    #[test]
    fn setup_triglavian_status_foreign_key_still_enforced() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        setup(&connection);
        setup_triglavian_status(&connection).unwrap();

        let result = connection.execute(
            "UPDATE mapSolarSystems SET trigStatusID = 99 WHERE solarSystemId = 30000001",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn setup_jove_observatories_marks_by_exact_name() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        setup_jove_observatories(&connection).unwrap();

        // "0-4VQL" (30000001) esta en la lista real.
        let marked: i64 = connection
            .query_row(
                "SELECT joveObservatory FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, 1);

        // "Sys Edencom" (30003088) no esta en la lista.
        let unmarked: i64 = connection
            .query_row(
                "SELECT joveObservatory FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unmarked, 0);
    }

    #[test]
    fn setup_special_anomalies_marks_a0_spectral_class_systems() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);
        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) VALUES (6,'Celestial',1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, categoryId, groupName, anchorable) VALUES (6,6,'Sun',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invTypes (typeId, groupId, typeName, published) VALUES (3000,6,'White A0 (ffffff)',1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO typeStar (typeId, name, color) VALUES (3000,'A0','ffffff')",
                [],
            )
            .unwrap();
        let star_type_id: i64 = connection
            .query_row("SELECT starTypeId FROM typeStar WHERE typeId = 3000", [], |row| row.get(0))
            .unwrap();
        connection
            .execute(
                "INSERT INTO mapStars (starId, solarSystemId, radius, starTypeId) \
                 VALUES (40000001, 30000001, 1000, ?1)",
                rusqlite::params![star_type_id],
            )
            .unwrap();

        setup_special_anomalies(&connection).unwrap();

        let marked: i64 = connection
            .query_row(
                "SELECT specialOreAnom FROM mapSolarSystems WHERE solarSystemId = 30000001",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marked, 1);

        let unmarked: i64 = connection
            .query_row(
                "SELECT specialOreAnom FROM mapSolarSystems WHERE solarSystemId = 30003088",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unmarked, 0);
    }

    #[test]
    fn update_tables_only_creates_what_config_enables() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        let config = DotlanConfig {
            with_icebelts: false,
            with_triglavian_status: false,
            with_jove_observatories: false,
            with_special_ore: false,
        };
        update_tables(&connection, &config).unwrap();

        // mapAbstractSystems corre siempre.
        let abstract_exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='mapAbstractSystems'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(abstract_exists, 1);

        // Ninguna de las 4 columnas opcionales debe existir.
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('mapSolarSystems')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for optional in ["iceBelt", "trigStatusID", "joveObservatory", "specialOreAnom"] {
            assert!(
                !columns.iter().any(|c| c == optional),
                "{optional} no deberia existir con todos los flags en false"
            );
        }
    }

    #[test]
    fn update_tables_creates_everything_when_all_enabled() {
        let connection = Connection::open_in_memory().unwrap();
        setup(&connection);

        let config = DotlanConfig {
            with_icebelts: true,
            with_triglavian_status: true,
            with_jove_observatories: true,
            with_special_ore: true,
        };
        update_tables(&connection, &config).unwrap();

        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('mapSolarSystems')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for expected in ["iceBelt", "trigStatusID", "joveObservatory", "specialOreAnom"] {
            assert!(columns.iter().any(|c| c == expected), "falta la columna {expected}");
        }
    }
}

