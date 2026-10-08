//! OS presentation only. The app's existing document owner prepares/authorizes
//! the private copy. No renderer body, destination or filesystem grant enters
//! this plugin, and presentation is never a remote delivery receipt.
mod accounts;
pub use accounts::{authenticate, cancel_authentication, vault_read, vault_write};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Mutex};
#[cfg(mobile)]
use tauri::Emitter;
use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Manager, Runtime,
};

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Export,
    Share,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Exported,
    Presented,
    Cancelled,
}
#[derive(Deserialize, Serialize)]
pub struct Reply {
    pub state: State,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyRequest {
    pub path: String,
    pub name: String,
    pub mime: String,
    pub sha256: String,
    pub bytes: u32,
    pub action: Action,
}
#[derive(Default)]
struct Links {
    pending: Mutex<(VecDeque<String>, bool)>,
}
#[derive(Serialize)]
pub struct PendingLinks {
    pub links: Vec<String>,
    pub truncated: bool,
}
#[cfg(mobile)]
fn receive<R: Runtime>(app: &AppHandle<R>, url: &str) {
    if url.len() > 16384
        || !url.starts_with("mdreader://read?")
        || url.chars().any(char::is_control)
    {
        return;
    }
    let Some(links) = app.try_state::<Links>() else {
        return;
    };
    if let Ok(mut queue) = links.pending.lock() {
        if queue.0.len() == 8 {
            queue.0.pop_front();
            queue.1 = true;
        }
        queue.0.push_back(url.into());
        let _ = app.emit("mdreader://reading-link-open", url);
    };
}
pub fn take_links<R: Runtime>(app: &AppHandle<R>) -> Result<PendingLinks, &'static str> {
    let links = app.try_state::<Links>().ok_or("unsupported")?;
    let mut queue = links.pending.lock().map_err(|_| "unavailable")?;
    let result = PendingLinks {
        links: queue.0.drain(..).collect(),
        truncated: queue.1,
    };
    queue.1 = false;
    Ok(result)
}
#[cfg(mobile)]
struct Platform<R: Runtime> {
    handle: tauri::plugin::PluginHandle<R>,
    links_ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_mdreader_transfer);

pub fn available<R: Runtime>(_app: &AppHandle<R>) -> bool {
    #[cfg(mobile)]
    {
        _app.try_state::<Platform<R>>().is_some()
    }
    #[cfg(not(mobile))]
    {
        false
    }
}
pub fn links_available<R: Runtime>(_app: &AppHandle<R>) -> bool {
    #[cfg(mobile)]
    {
        _app.try_state::<Platform<R>>()
            .is_some_and(|p| p.links_ready.load(std::sync::atomic::Ordering::Acquire))
    }
    #[cfg(not(mobile))]
    {
        false
    }
}
pub fn present<R: Runtime>(
    app: &AppHandle<R>,
    request: CopyRequest,
) -> Result<Reply, &'static str> {
    #[cfg(mobile)]
    {
        app.try_state::<Platform<R>>()
            .ok_or("unsupported")?
            .handle
            .run_mobile_plugin("presentCopy", request)
            .map_err(|_| "transfer_unconfirmed")
    }
    #[cfg(not(mobile))]
    {
        let _ = (app, request);
        Err("unsupported")
    }
}
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("mdreader-transfer")
        .setup(|app, _api| {
            app.manage(Links::default());
            #[cfg(mobile)]
            {
                #[cfg(target_os = "android")]
                let Ok(handle) =
                    _api.register_android_plugin("dev.mdreader.transfer", "TransferPlugin")
                else {
                    return Ok(());
                };
                #[cfg(target_os = "ios")]
                let Ok(handle) = _api.register_ios_plugin(init_plugin_mdreader_transfer) else {
                    return Ok(());
                };
                let links_ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(cfg!(
                    target_os = "ios"
                )));
                #[cfg(target_os = "android")]
                {
                    let app = app.clone();
                    let receiver = tauri::ipc::Channel::<serde_json::Value>::new(move |body| {
                        if let tauri::ipc::InvokeResponseBody::Json(text) = body {
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                                if let Some(url) =
                                    value.get("url").and_then(serde_json::Value::as_str)
                                {
                                    receive(&app, url);
                                }
                            }
                        }
                        Ok(())
                    });
                    let native = handle.clone();
                    let ready = links_ready.clone();
                    tauri::async_runtime::spawn(async move {
                        let result: Result<serde_json::Value, _> = native
                            .run_mobile_plugin_async(
                                "watchLinks",
                                serde_json::json!({ "handler": receiver }),
                            )
                            .await;
                        ready.store(result.is_ok(), std::sync::atomic::Ordering::Release);
                    });
                }
                app.manage(Platform {
                    handle,
                    links_ready,
                });
            }
            Ok(())
        })
        .on_event(|_app, _event| {
            #[cfg(target_os = "ios")]
            if let tauri::RunEvent::Opened { urls } = _event {
                for url in urls {
                    receive(_app, url.as_str());
                }
            }
        })
        .build()
}
