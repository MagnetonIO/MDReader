//! Private Android selected-stream queue. Renderer commands can request a drain,
//! but receive neither the selected URI, private path nor unimported body.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Default)]
pub(super) struct Incoming {
    busy: Arc<AtomicBool>,
}
pub struct IncomingLease(Arc<AtomicBool>);
impl Drop for IncomingLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncomingItem {
    pub id: String,
    pub name: Option<String>,
    pub bytes: u32,
    pub base64: Option<String>,
    pub failure: Option<String>,
    #[serde(default)]
    pub open: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncomingQueue {
    pub item: Option<IncomingItem>,
    pub remaining: u32,
}
pub fn claim_incoming<R: Runtime>(app: &AppHandle<R>) -> Result<IncomingLease, &'static str> {
    let state = app.try_state::<Incoming>().ok_or("unsupported")?;
    state
        .busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "busy")?;
    Ok(IncomingLease(state.busy.clone()))
}
pub async fn next_incoming<R: Runtime>(app: &AppHandle<R>) -> Result<IncomingQueue, &'static str> {
    #[cfg(target_os = "android")]
    {
        app.try_state::<Platform<R>>()
            .ok_or("unsupported")?
            .handle
            .run_mobile_plugin_async("nextIncomingMarkdown", serde_json::json!({}))
            .await
            .map_err(|_| "unavailable")
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("unsupported")
    }
}
pub async fn acknowledge_incoming<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
) -> Result<(), &'static str> {
    #[cfg(target_os = "android")]
    {
        let _: serde_json::Value = app
            .try_state::<Platform<R>>()
            .ok_or("unsupported")?
            .handle
            .run_mobile_plugin_async("acknowledgeIncomingMarkdown", serde_json::json!({"id":id}))
            .await
            .map_err(|_| "unavailable")?;
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, id);
        Err("unsupported")
    }
}
