//! Native-only account transport. No renderer command handler is registered.
//! Callback/code and encrypted-vault plaintext return directly to Rust custody.
use super::*;
#[cfg(mobile)]
use zeroize::Zeroize;
use zeroize::Zeroizing;
#[cfg(mobile)]
#[derive(Deserialize)]
struct AuthReply {
    state: String,
    #[serde(default)]
    url: String,
}
#[cfg(mobile)]
impl Drop for AuthReply {
    fn drop(&mut self) {
        self.url.zeroize();
    }
}
pub async fn authenticate<R: Runtime>(
    app: &AppHandle<R>,
    attempt: &str,
    authorization: &str,
    redirect: &str,
) -> Result<Zeroizing<String>, &'static str> {
    #[cfg(mobile)]
    {
        let reply: AuthReply = app.try_state::<Platform<R>>().ok_or("unsupported")?.handle
            .run_mobile_plugin_async("authenticate", serde_json::json!({"attempt":attempt,"authorization":authorization,"redirect":redirect})).await.map_err(|_| "interrupted")?;
        if reply.state == "cancelled" {
            return Err("cancelled");
        }
        if reply.state != "completed" || reply.url.len() > 16384 {
            return Err("interrupted");
        }
        Ok(Zeroizing::new(reply.url.clone()))
    }
    #[cfg(not(mobile))]
    {
        let _ = (app, attempt, authorization, redirect);
        Err("unsupported")
    }
}
pub async fn cancel_authentication<R: Runtime>(app: &AppHandle<R>, attempt: &str) {
    #[cfg(mobile)]
    if let Some(platform) = app.try_state::<Platform<R>>() {
        let _: Result<serde_json::Value, _> = platform
            .handle
            .run_mobile_plugin_async(
                "cancelAuthentication",
                serde_json::json!({"attempt":attempt}),
            )
            .await;
    }
    #[cfg(not(mobile))]
    {
        let _ = (app, attempt);
    }
}
#[cfg(target_os = "android")]
#[derive(Deserialize)]
struct VaultReply {
    #[serde(default)]
    bytes: Option<String>,
}
#[cfg(target_os = "android")]
impl Drop for VaultReply {
    fn drop(&mut self) {
        if let Some(value) = self.bytes.as_mut() {
            value.zeroize();
        }
    }
}
pub fn vault_read<R: Runtime>(
    app: &AppHandle<R>,
    namespace: &str,
    reference: &str,
) -> Result<Option<Zeroizing<String>>, &'static str> {
    #[cfg(target_os = "android")]
    {
        let value: VaultReply = app
            .try_state::<Platform<R>>()
            .ok_or("unsupported")?
            .handle
            .run_mobile_plugin(
                "vaultRead",
                serde_json::json!({"namespace":namespace,"reference":reference}),
            )
            .map_err(|_| "custody")?;
        if value.bytes.as_ref().is_some_and(|b| b.len() > 64 * 1024) {
            return Err("custody");
        }
        Ok(value.bytes.as_ref().map(|b| Zeroizing::new(b.clone())))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, namespace, reference);
        Err("unsupported")
    }
}
pub fn vault_write<R: Runtime>(
    app: &AppHandle<R>,
    namespace: &str,
    reference: &str,
    bytes: &str,
) -> Result<(), &'static str> {
    #[cfg(target_os = "android")]
    {
        if bytes.len() > 64 * 1024 {
            return Err("custody");
        }
        let _: serde_json::Value = app
            .try_state::<Platform<R>>()
            .ok_or("unsupported")?
            .handle
            .run_mobile_plugin(
                "vaultWrite",
                serde_json::json!({"namespace":namespace,"reference":reference,"bytes":bytes}),
            )
            .map_err(|_| "custody")?;
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, namespace, reference, bytes);
        Err("unsupported")
    }
}
