//! Creación del schema STRICT de `sde.db`.
//!
//! Equivalente a `SdeParser.create_table_structure()` en el prototipo
//! Python (`sde_parser.py`), que abre `schema_corregido_strict.sql` desde
//! disco y lo ejecuta contra la conexión con `cur.executescript(query)`.
//!
//! Aquí el DDL se embebe en el binario en tiempo de compilación con
//! `include_str!` en vez de leerse desde el filesystem en tiempo de
//! ejecución: el builder de `sde` no depende de que un archivo `.sql`
//! externo exista en un path relativo dado (frágil si el binario corre
//! desde un directorio de trabajo distinto), y el DDL queda versionado
//! junto con el código que lo consume.
//!
//! `schema.sql` es una copia 1:1 de `schema_corregido_strict.sql` del
//! proyecto `databaseCreator` (la fuente de verdad original del schema);
//! si ese archivo cambia, esta copia debe actualizarse a mano.
//!
//! Nota sobre "escritura final a sqlite": este módulo solo cubre la
//! creación del schema vacío (DDL). Poblarlo con datos reales del SDE
//! (el equivalente a los ~15 métodos `_parse_*` de `sde_parser.py`:
//! categorías, grupos, tipos, razas, facciones, regiones, constelaciones,
//! sistemas solares con proyección isométrica/dimétrica, stargates,
//! estrellas, planetas, lunas y conexiones) es un trabajo bastante más
//! grande y queda fuera de este módulo -- ver `builder::mod` para el
//! resto de piezas pendientes del builder.

use rusqlite::Connection;

/// El DDL STRICT completo (tablas, índices, FKs) para `sde.db`.
///
/// Mantenido en sincronía manualmente con `schema_corregido_strict.sql`
/// del proyecto `databaseCreator` -- ver ese archivo para el historial de
/// cambios y las notas sobre el rework del SDE de sept. 2025.
pub const SCHEMA_DDL: &str = include_str!("schema.sql");

/// Crea el schema STRICT completo en `connection`, ejecutando
/// [`SCHEMA_DDL`] de una sola vez.
///
/// La primera sentencia del DDL es `PRAGMA foreign_keys = ON;`, así que
/// tras esta llamada la conexión queda con FK enforcement activo para el
/// resto de su vida (a menos que algo más la desactive explícitamente) --
/// no hace falta volver a setear esa pragma antes de insertar datos sobre
/// la misma `connection`.
///
/// El DDL no usa `CREATE TABLE IF NOT EXISTS` a propósito: llamar a esta
/// función dos veces sobre la misma conexión falla (`table X already
/// exists`) en vez de mezclarse silenciosamente con un schema parcial o
/// corrupto de una corrida anterior. El llamador es responsable de pasar
/// una conexión "limpia" (base de datos nueva o vacía).
pub fn create_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(SCHEMA_DDL)
}

/// Nombres de las tablas declaradas por [`SCHEMA_DDL`], en el orden en que
/// aparecen.
///
/// Parseo deliberadamente simple (una pasada por línea buscando el
/// prefijo `CREATE TABLE `): el DDL es un recurso estático y de confianza
/// del propio crate, no input externo, así que no vale la pena traer un
/// parser SQL real solo para esto. Sirve para verificaciones posteriores
/// a la creación (p. ej. en tests) sin mantener una lista manual aparte
/// que se pueda desincronizar del DDL real.
pub fn table_names() -> Vec<&'static str> {
    SCHEMA_DDL
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("CREATE TABLE "))
        .map(|rest| rest.split_whitespace().next().unwrap_or(""))
        .filter(|name| !name.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_schema_succeeds_on_fresh_connection() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();
    }

    #[test]
    fn create_schema_creates_every_declared_table() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();

        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let mut existing: Vec<String> = statement
            .query_map([], |row| row.get::<usize, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        existing.sort();

        let mut expected: Vec<String> = table_names().iter().map(|s| s.to_string()).collect();
        expected.sort();

        assert_eq!(existing, expected);
        // Ancla adicional: si este número cambia, seguramente cambió el
        // DDL y vale la pena revisar el resto de este archivo de tests.
        assert_eq!(expected.len(), 19);
    }

    #[test]
    fn create_schema_fails_if_called_twice() {
        // El DDL no usa `CREATE TABLE IF NOT EXISTS` a propósito: una
        // segunda llamada sobre la misma conexión debe fallar de forma
        // visible, no mezclarse en silencio con un schema anterior.
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();
        assert!(create_schema(&connection).is_err());
    }

    #[test]
    fn create_schema_enforces_strict_typing() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();

        // invCategories.published es `INTEGER NOT NULL CHECK (published
        // IN (0,1))`; en una tabla STRICT, insertar un TEXT en una columna
        // INTEGER debe fallar directamente en vez de convertirse en
        // silencio por afinidad (comportamiento SQLite estándar, no
        // STRICT).
        let result = connection.execute(
            "INSERT INTO invCategories (categoryId, categoryName, published) \
             VALUES (1, 'Test', 'yes')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn create_schema_enforces_foreign_keys() {
        // El propio DDL activa `PRAGMA foreign_keys = ON` como primera
        // sentencia, así que no hace falta activarla aparte antes de
        // llamar a create_schema().
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();

        // invGroups.categoryId referencia invCategories(categoryId); no
        // existe ninguna categoría con id 999, así que este INSERT debe
        // ser rechazado.
        let result = connection.execute(
            "INSERT INTO invGroups (groupId, groupName, categoryId, anchorable) \
             VALUES (1, 'Test', 999, 0)",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn create_schema_accepts_valid_rows_respecting_fk_order() {
        // Prueba de humo end-to-end: una fila válida en cada extremo de
        // una relación FK (invCategories -> invGroups) debe insertarse
        // sin problemas cuando el padre se crea primero.
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();

        connection
            .execute(
                "INSERT INTO invCategories (categoryId, categoryName, published) \
                 VALUES (1, 'Celestial', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO invGroups (groupId, groupName, categoryId, anchorable) \
                 VALUES (10, 'Sun', 1, 0)",
                [],
            )
            .unwrap();

        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM invGroups", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
