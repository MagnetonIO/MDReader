//! Direct-distribution updater commands.
//!
//! The command names are always present so the generated frontend contract is
//! stable. The capability itself is compile-time gated: store, E2E, mobile, and
//! ordinary development builds return `false` and cannot construct an updater.

use serde::Serialize;
use specta::Type;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn updater_available() -> bool {
    cfg!(feature = "direct-updater") && !cfg!(mobile)
}

#[tauri::command]
#[specta::specta]
pub async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    check_for_update_inner(app).await
}

#[cfg(all(feature = "direct-updater", not(mobile)))]
async fn check_for_update_inner(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;

    let update = app
        .updater()
        .map_err(|error| format!("updater is not configured: {error}"))?
        .check()
        .await
        .map_err(|error| format!("update check failed: {error}"))?;
    Ok(update.map(|update| UpdateInfo {
        version: update.version,
        current_version: update.current_version,
        notes: update.body,
    }))
}

#[cfg(not(all(feature = "direct-updater", not(mobile))))]
async fn check_for_update_inner(_app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    Err("updates are unavailable in this distribution channel".to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn install_update(app: tauri::AppHandle, expected_version: String) -> Result<(), String> {
    install_update_inner(app, expected_version).await
}

#[cfg(all(feature = "direct-updater", not(mobile)))]
async fn download_verified(update: &tauri_plugin_updater::Update) -> Result<Vec<u8>, String> {
    update
        .download(|_, _| {}, || {})
        .await
        .map_err(|error| format!("update download or signature verification failed: {error}"))
}

#[cfg(all(feature = "direct-updater", not(mobile)))]
async fn install_update_inner(
    app: tauri::AppHandle,
    expected_version: String,
) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;

    let update = app
        .updater()
        .map_err(|error| format!("updater is not configured: {error}"))?
        .check()
        .await
        .map_err(|error| format!("update check failed: {error}"))?
        .ok_or_else(|| "the selected update is no longer available".to_string())?;
    if update.version != expected_version {
        return Err(format!(
            "available update changed from {expected_version} to {} — review it before installing",
            update.version
        ));
    }
    let bytes = download_verified(&update).await?;
    update
        .install(bytes)
        .map_err(|error| format!("update install failed: {error}"))
}

#[cfg(not(all(feature = "direct-updater", not(mobile))))]
async fn install_update_inner(
    _app: tauri::AppHandle,
    _expected_version: String,
) -> Result<(), String> {
    Err("updates are unavailable in this distribution channel".to_string())
}

#[cfg(all(test, feature = "direct-updater", not(mobile)))]
mod tests {
    use super::download_verified;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    use tauri_plugin_updater::UpdaterExt;

    const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXkgRTc2MjBGMTg0MkI0RTgxRgpSV1FmNkxSQ0dBOWk1M21sWWVjTzRJelQ1MVRHUHB2V3VjTlNDaDFDQk0wUVRhTG43M1k3R0ZPMwo=";
    const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIG1pbmlzaWduIHNlY3JldCBrZXkKUldRZjZMUkNHQTlpNTlTTE9GeHo2Tnh2QVNYREplUnR1Wnlrd1FlcGJERUd0ODdpZzFCTnBXYVZXdU5ybTczWWlJaUpicTcxV2krZFA5ZUtMOE9DMzUxdndJYXNTU2JYeHdBPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNTU1Nzc5OTY2CWZpbGU6dGVzdApRdEtNWFd5WWN3ZHBaQWxQRjd0RTJFTkprUmQxdWp2S2psajFtOVJ0SFRCblpQYTVXS1U1dVdSczVHb1A1TS9WcUU4MVFGdU1LSTVrL1NmTlFVYU9BQT09Cg==";

    async fn fixture_download(body: &'static [u8]) -> Result<Vec<u8>, String> {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let address = listener.local_addr().expect("fixture address");
        let manifest = format!(
            r#"{{"version":"1.0.0","url":"http://{address}/artifact","signature":"{SIGNATURE}"}}"#
        );
        let server = thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.expect("fixture connection");
                let mut request = [0_u8; 2048];
                let length = stream.read(&mut request).expect("read fixture request");
                let request = String::from_utf8_lossy(&request[..length]);
                let (content_type, body) = if request.starts_with("GET /manifest ") {
                    ("application/json", manifest.as_bytes())
                } else {
                    ("application/octet-stream", body)
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .expect("write fixture headers");
                stream.write_all(body).expect("write fixture body");
            }
        });

        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".to_string(),
            serde_json::json!({ "pubkey": PUBLIC_KEY, "endpoints": [] }),
        );
        let app = tauri::test::mock_builder()
            .plugin(
                tauri_plugin_updater::Builder::new()
                    .pubkey(PUBLIC_KEY)
                    .build(),
            )
            .build(context)
            .expect("build updater test app");
        let updater = app
            .handle()
            .updater_builder()
            .endpoints(vec![format!("http://{address}/manifest")
                .parse()
                .expect("fixture URL")])
            .expect("set fixture endpoint")
            .build()
            .expect("build updater");
        let update = updater
            .check()
            .await
            .expect("check fixture")
            .expect("fixture is newer");

        let result = download_verified(&update).await;
        server.join().expect("fixture server");
        result
    }

    #[tokio::test]
    async fn production_download_service_accepts_only_the_signed_fixture_bytes() {
        assert_eq!(
            fixture_download(b"test").await.expect("verify fixture"),
            b"test"
        );
        assert!(
            fixture_download(b"Test").await.is_err(),
            "changing the downloaded bytes must fail the production updater verifier"
        );
    }
}
