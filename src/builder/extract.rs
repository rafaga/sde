//! Descompresión del zip del SDE en el directorio de trabajo del
//! builder, preservando `maps/` (los SVG de dotlan, que vienen de otra
//! fuente y no deben perderse ni re-descargarse en cada build).
//! Equivalente a la parte de `MiscUtils.zip_decompress()` +  la limpieza
//! de `sde_path` que hace `database_builder.py` antes de invocar al
//! parser.
//!
//! A diferencia de Python (que usa `zipfile.ZipFile(...).extractall()`),
//! acá se usa `ZipArchive::extract()`, que ya viene incorporada en el
//! propio crate `zip` (versión 8.x) -- no hizo falta ningún crate
//! auxiliar como `zip-extensions`.

use crate::builder::BuilderError;
use std::path::Path;

/// Descomprime `zip_path` en `destination` (lo crea si no existe).
/// Sobrescribe archivos existentes -- mismo comportamiento que
/// `zipfile.ZipFile.extractall()` en Python. Equivalente a
/// `MiscUtils.zip_decompress()`, salvo que acá un zip corrupto/inválido
/// se propaga como `Err` (vía `BuilderError::Zip`) en vez de devolver
/// `false` -- consistente con el resto de este módulo (`extract_map_data`
/// es la única función que distingue "falla recuperable, reintentar
/// descarga" de "error genuino" con un `bool`, porque ahí forma parte de
/// un flujo de reintento explícito; acá no hay reintento posible dentro
/// de esta función, así que un `Err` directo es más claro que un `bool`
/// que el caller tendría que interpretar).
pub fn unzip(zip_path: &Path, destination: &Path) -> Result<(), BuilderError> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    archive.extract(destination)?;
    Ok(())
}

/// Vacía `sde_dir` (si ya existe) preservando `<sde_dir>/maps/` tal
/// cual. No hace nada si `sde_dir` todavía no existe -- no hay nada que
/// limpiar. Equivalente al loop `for item in sde_path.iterdir(): ...` de
/// `database_builder.py`.
///
/// A diferencia de Python (que compara `item.resolve() ==
/// maps_path.resolve()`, canonicalizando ambos paths), acá se compara
/// directamente sin canonicalizar: alcanza porque tanto la entrada de
/// `read_dir` como `maps_dir` se construyen a partir del mismo
/// `sde_dir` de la misma forma, y canonicalizar `maps_dir` de antemano
/// fallaría si esa carpeta todavía no existe (un caso perfectamente
/// válido -- p. ej. la primera vez que se corre el builder, antes de
/// que `dotlan::process()` descargue ningún mapa).
pub fn clean_except_maps(sde_dir: &Path) -> Result<(), BuilderError> {
    if !sde_dir.exists() {
        return Ok(());
    }
    let maps_dir = sde_dir.join("maps");
    for entry in std::fs::read_dir(sde_dir)? {
        let path = entry?.path();
        if path == maps_dir {
            continue;
        }
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Limpia `sde_dir` preservando `maps/` y descomprime `zip_path` ahí --
/// composición directa de [`clean_except_maps`] seguido de [`unzip`], en
/// ese orden, el mismo orden que usa `database_builder.py`.
pub fn prepare_sde_directory(zip_path: &Path, sde_dir: &Path) -> Result<(), BuilderError> {
    clean_except_maps(sde_dir)?;
    unzip(zip_path, sde_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sde-extract-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Crea un zip válido en `path` con las entradas dadas
    /// `(nombre, contenido)`, incluyendo subcarpetas si el nombre trae
    /// `/`. Usa `SimpleFileOptions` -- el tipo que la propia
    /// documentación de `zip` 8.6.0 recomienda para el caso simple (sin
    /// especificar a mano el parámetro genérico de `FileOptions`).
    fn build_test_zip(path: &Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn unzip_extracts_files_and_subdirectories() {
        let dir = temp_dir("basic");
        let zip_path = dir.join("test.zip");
        build_test_zip(
            &zip_path,
            &[
                ("types.jsonl", "{\"_key\": 1}\n"),
                ("universe/regions/The_Forge.jsonl", "{\"_key\": 10000002}\n"),
            ],
        );
        let destination = dir.join("out");

        unzip(&zip_path, &destination).unwrap();

        let types_content = std::fs::read_to_string(destination.join("types.jsonl")).unwrap();
        assert_eq!(types_content, "{\"_key\": 1}\n");
        let region_content =
            std::fs::read_to_string(destination.join("universe/regions/The_Forge.jsonl")).unwrap();
        assert_eq!(region_content, "{\"_key\": 10000002}\n");
    }

    #[test]
    fn unzip_errors_on_invalid_zip() {
        let dir = temp_dir("invalid");
        let bad_zip = dir.join("not_a_zip.zip");
        std::fs::write(&bad_zip, b"esto no es un zip").unwrap();
        let destination = dir.join("out");

        let result = unzip(&bad_zip, &destination);
        assert!(matches!(result, Err(BuilderError::Zip(_))));
    }

    #[test]
    fn clean_except_maps_does_nothing_when_directory_missing() {
        let dir = temp_dir("missing");
        let sde_dir = dir.join("no_existe_todavia");
        // no debe fallar aunque sde_dir no exista
        clean_except_maps(&sde_dir).unwrap();
    }

    #[test]
    fn clean_except_maps_preserves_maps_removes_the_rest() {
        let dir = temp_dir("preserve");
        let sde_dir = dir.join("sde");
        std::fs::create_dir_all(sde_dir.join("maps")).unwrap();
        std::fs::write(sde_dir.join("maps").join("The_Forge.svg"), "svg viejo").unwrap();
        std::fs::write(sde_dir.join("types.jsonl"), "datos viejos").unwrap();
        std::fs::create_dir_all(sde_dir.join("universe")).unwrap();
        std::fs::write(sde_dir.join("universe").join("region.jsonl"), "mas datos viejos").unwrap();

        clean_except_maps(&sde_dir).unwrap();

        assert!(sde_dir.join("maps").exists(), "maps/ debe sobrevivir");
        assert!(
            sde_dir.join("maps").join("The_Forge.svg").exists(),
            "el contenido de maps/ tampoco debe tocarse"
        );
        assert!(!sde_dir.join("types.jsonl").exists(), "types.jsonl debe borrarse");
        assert!(!sde_dir.join("universe").exists(), "universe/ debe borrarse completa");
    }

    #[test]
    fn prepare_sde_directory_cleans_then_extracts() {
        let dir = temp_dir("prepare");
        let sde_dir = dir.join("sde");
        std::fs::create_dir_all(sde_dir.join("maps")).unwrap();
        std::fs::write(sde_dir.join("maps").join("Domain.svg"), "svg preservado").unwrap();
        std::fs::write(sde_dir.join("old_data.jsonl"), "datos de un build anterior").unwrap();

        let zip_path = dir.join("new_sde.zip");
        build_test_zip(&zip_path, &[("types.jsonl", "{\"_key\": 1}\n")]);

        prepare_sde_directory(&zip_path, &sde_dir).unwrap();

        assert!(!sde_dir.join("old_data.jsonl").exists());
        assert!(sde_dir.join("maps").join("Domain.svg").exists());
        let types_content = std::fs::read_to_string(sde_dir.join("types.jsonl")).unwrap();
        assert_eq!(types_content, "{\"_key\": 1}\n");
    }
}
