//! Chequeo del build number más reciente del SDE publicado por CCP
//! (`developers.eveonline.com`), y descarga condicional del zip
//! correspondiente. Equivalente a `update_as_needed()` en
//! `database_builder.py`.
//!
//! Igual que [`super::http`], este módulo no escribe nada a la base de
//! datos -- solo maneja archivos en disco (`latest.jsonl`, el `.build`
//! con el número guardado localmente, y el zip en sí). Portar el
//! `.zip` descargado hacia adentro del árbol de trabajo del builder
//! (preservando `maps/`, ver [`super::manifest::manifest_path`]) queda
//! para `builder::extract`, todavía sin portar.

use crate::builder::http;
use crate::builder::BuilderError;
use reqwest::Client;
use std::path::Path;

/// Busca, línea por línea, el registro de `latest.jsonl` con
/// `_key == "sde"` y devuelve su `buildNumber` como texto.
///
/// Verificado contra un `latest.jsonl` real (agosto 2026): trae una sola
/// línea (`{"_key": "sde", "buildNumber": 3458726, "releaseDate":
/// "..."}`, con final de línea `\r\n`) -- no varias, una por dataset,
/// como especulaba antes de tener una muestra real. `buildNumber` viene
/// como número JSON (no como texto entrecomillado), y `str::lines()`
/// (usado acá) maneja el `\r\n` correctamente sin dejar un `\r` colgado
/// que rompa el parseo JSON -- confirmado línea por línea contra el
/// archivo real, byte a byte.
///
/// El parseo sigue siendo deliberadamente tolerante más allá de este
/// caso confirmado: cualquier línea que no sea JSON válido, o que no
/// tenga el campo esperado, simplemente se salta en vez de abortar el
/// archivo entero -- por si alguna vez trae más de una línea, o el
/// formato cambia -- mismo espíritu defensivo que Python, que usa
/// `.get()` con `None` por todos lados acá en vez de indexado directo.
///
/// `buildNumber` se devuelve como `String` sin importar si en el JSON
/// original es un número o ya viene como texto -- igual que Python, que
/// compara builds con `str(current) == str(latest)` en vez de aritmética
/// (el build number es un identificador opaco, no algo con lo que se
/// vaya a operar numéricamente).
fn find_sde_build_number(jsonl: &str) -> Option<String> {
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if record.get("_key").and_then(|v| v.as_str()) != Some("sde") {
            continue;
        }
        let build = record.get("buildNumber")?;
        return Some(match build {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        });
    }
    None
}

/// Verifica el build number más reciente del SDE
/// (`{sde_url_base}latest.jsonl`) y descarga
/// `eve-online-static-data-{build}-{variant}.zip` a
/// `<data_dir>/sde-{variant}.zip` solo si es más nuevo que el guardado
/// localmente en `<data_dir>/sde-{variant}.build`. Equivalente a
/// `update_as_needed()` en Python.
///
/// `sde_url_base` debe terminar en `/` (p. ej.
/// `"https://developers.eveonline.com/static-data/tranquility/"`).
/// `variant` es `"jsonl"` o `"yaml"` -- se usa tal cual en los nombres de
/// archivo, sin validar contra un enum cerrado: si CCP agrega un tercer
/// formato de exportación, esta función no necesita cambios.
///
/// Devuelve `Ok(true)` si se descargó una versión nueva, `Ok(false)` si
/// ya estaba actualizado -- o si no se pudo determinar el build remoto
/// (sin red, `latest.jsonl` no trae el registro esperado, etc.): mismo
/// comportamiento que Python, que también devuelve `False` en ese caso
/// en vez de propagar una excepción, para no bloquear el build entero
/// por un problema puntual verificando la versión.
///
/// # Mejora deliberada sobre Python: descarga a temporal + rename
///
/// Python borra el zip viejo *antes* de intentar descargar el nuevo
/// (`if zip_file.exists(): zip_file.unlink()`, luego recién
/// `download_control(zip_url)`) -- si la descarga falla a mitad de
/// camino, esa corrida se queda sin zip viejo NI nuevo. Acá se descarga
/// primero a un archivo temporal (`sde-{variant}.zip.tmp`) y solo se
/// reemplaza `sde-{variant}.zip` (vía `rename`, atómico en el mismo
/// filesystem) una vez que la descarga terminó con éxito -- si falla,
/// el zip anterior queda intacto.
pub async fn update_as_needed(
    client: &Client,
    data_dir: &Path,
    sde_url_base: &str,
    variant: &str,
) -> Result<bool, BuilderError> {
    std::fs::create_dir_all(data_dir)?;

    let build_file = data_dir.join(format!("sde-{variant}.build"));
    let zip_file = data_dir.join(format!("sde-{variant}.zip"));
    let temp_zip_file = data_dir.join(format!("sde-{variant}.zip.tmp"));

    let index_url = format!("{sde_url_base}latest.jsonl");
    let index_contents = match http::fetch_text(client, &index_url).await {
        Ok(contents) => contents,
        Err(err) => {
            eprintln!("sde_index: no se pudo descargar {index_url} ({err})");
            return Ok(false);
        }
    };

    let Some(latest_build) = find_sde_build_number(&index_contents) else {
        eprintln!("sde_index: no se pudo determinar el build number más reciente en {index_url}");
        return Ok(false);
    };

    let current_build = std::fs::read_to_string(&build_file)
        .ok()
        .map(|s| s.trim().to_string());

    if current_build.as_deref() == Some(latest_build.as_str()) && zip_file.exists() {
        println!("sde_index: datos {variant} ya actualizados (build {latest_build})");
        return Ok(false);
    }

    println!(
        "sde_index: nuevo build disponible ({} -> {latest_build}), descargando datos {variant}",
        current_build.as_deref().unwrap_or("ninguno")
    );

    let zip_url = format!("{sde_url_base}eve-online-static-data-{latest_build}-{variant}.zip");
    http::download(client, &zip_url, &temp_zip_file, |_| {}).await?;
    std::fs::rename(&temp_zip_file, &zip_file)?;
    std::fs::write(&build_file, &latest_build)?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_sde_build_number_extracts_matching_key() {
        let jsonl = "{\"_key\": \"sde\", \"buildNumber\": 12345}\n";
        assert_eq!(find_sde_build_number(jsonl), Some("12345".to_string()));
    }

    #[test]
    fn find_sde_build_number_ignores_other_keys() {
        let jsonl = concat!(
            "{\"_key\": \"universe\", \"buildNumber\": 99999}\n",
            "{\"_key\": \"sde\", \"buildNumber\": 12345}\n",
            "{\"_key\": \"bsd\", \"buildNumber\": 11111}\n",
        );
        assert_eq!(find_sde_build_number(jsonl), Some("12345".to_string()));
    }

    #[test]
    fn find_sde_build_number_returns_none_when_missing() {
        let jsonl = "{\"_key\": \"universe\", \"buildNumber\": 99999}\n";
        assert_eq!(find_sde_build_number(jsonl), None);
    }

    #[test]
    fn find_sde_build_number_skips_malformed_lines_without_aborting() {
        let jsonl = concat!(
            "no es json valido\n",
            "\n", // linea vacia
            "{\"_key\": \"sde\", \"buildNumber\": 42}\n",
        );
        assert_eq!(find_sde_build_number(jsonl), Some("42".to_string()));
    }

    #[test]
    fn find_sde_build_number_accepts_string_build_numbers() {
        // por si en la practica CCP alguna vez trae buildNumber como texto
        let jsonl = "{\"_key\": \"sde\", \"buildNumber\": \"12345\"}\n";
        assert_eq!(find_sde_build_number(jsonl), Some("12345".to_string()));
    }

    #[test]
    fn find_sde_build_number_handles_real_latest_jsonl() {
        // Contenido EXACTO de un latest.jsonl real (agosto 2026), con su
        // \r\n de final de linea tal cual viene -- no sintetizado a mano.
        let jsonl = "{\"_key\": \"sde\", \"buildNumber\": 3458726, \"releaseDate\": \"2026-08-06T11:07:36Z\"}\r\n";
        assert_eq!(find_sde_build_number(jsonl), Some("3458726".to_string()));
    }

    fn temp_data_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sde-index-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn update_as_needed_downloads_on_first_run() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/latest.jsonl"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("{\"_key\": \"sde\", \"buildNumber\": 123}\n"),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/eve-online-static-data-123-jsonl.zip",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(b"contenido del zip".to_vec()))
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("first_run");
        let base_url = format!("{}/", server.uri());

        let changed = update_as_needed(&client, &data_dir, &base_url, "jsonl")
            .await
            .unwrap();
        assert!(changed);

        let build = std::fs::read_to_string(data_dir.join("sde-jsonl.build")).unwrap();
        assert_eq!(build, "123");
        let zip_contents = std::fs::read(data_dir.join("sde-jsonl.zip")).unwrap();
        assert_eq!(zip_contents, b"contenido del zip");
        assert!(!data_dir.join("sde-jsonl.zip.tmp").exists());
    }

    #[tokio::test]
    async fn update_as_needed_skips_when_build_matches() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/latest.jsonl"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("{\"_key\": \"sde\", \"buildNumber\": 123}\n"),
            )
            .mount(&server)
            .await;
        // El zip nunca deberia pedirse -- 0 esperado explicitamente.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/eve-online-static-data-123-jsonl.zip",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("matches");
        std::fs::write(data_dir.join("sde-jsonl.build"), "123").unwrap();
        std::fs::write(data_dir.join("sde-jsonl.zip"), b"zip previo").unwrap();
        let base_url = format!("{}/", server.uri());

        let changed = update_as_needed(&client, &data_dir, &base_url, "jsonl")
            .await
            .unwrap();
        assert!(!changed);

        // El zip previo no debe haberse tocado.
        let zip_contents = std::fs::read(data_dir.join("sde-jsonl.zip")).unwrap();
        assert_eq!(zip_contents, b"zip previo");
    }

    #[tokio::test]
    async fn update_as_needed_downloads_when_build_changed() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/latest.jsonl"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("{\"_key\": \"sde\", \"buildNumber\": 456}\n"),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/eve-online-static-data-456-jsonl.zip",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(b"zip nuevo".to_vec()))
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("changed");
        std::fs::write(data_dir.join("sde-jsonl.build"), "123").unwrap();
        std::fs::write(data_dir.join("sde-jsonl.zip"), b"zip viejo").unwrap();
        let base_url = format!("{}/", server.uri());

        let changed = update_as_needed(&client, &data_dir, &base_url, "jsonl")
            .await
            .unwrap();
        assert!(changed);

        let build = std::fs::read_to_string(data_dir.join("sde-jsonl.build")).unwrap();
        assert_eq!(build, "456");
        let zip_contents = std::fs::read(data_dir.join("sde-jsonl.zip")).unwrap();
        assert_eq!(zip_contents, b"zip nuevo");
    }

    #[tokio::test]
    async fn update_as_needed_returns_false_when_index_unreachable() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/latest.jsonl"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = http::build_client().unwrap();
        let data_dir = temp_data_dir("unreachable");
        let base_url = format!("{}/", server.uri());

        let changed = update_as_needed(&client, &data_dir, &base_url, "jsonl")
            .await
            .unwrap();
        assert!(!changed);
    }
}
