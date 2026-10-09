//! Explicit real-model owner acceptance; not part of ordinary unit tests.
//! Usage: cargo run -p mdreader-service --features local-embed --example
//! embedded_semantic -- /ABS/PUBLIC_BGE_CACHE
#[cfg(all(feature = "local-embed", unix))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use mdreader_core::{device, ReaderStore};
    use mdreader_service::{EmbeddedRuntime, SearchMode, ServiceOptions};
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    let cache = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("explicit existing public BGE model cache required")?,
    );
    if !cache.is_absolute() || !cache.is_dir() {
        return Err("invalid public model cache".into());
    }
    let library = tempfile::tempdir()?;
    // Reuse only explicitly selected public model bytes, never a user library.
    std::os::unix::fs::symlink(cache, library.path().join("models"))?;
    let mut writer = ReaderStore::open(library.path())?;
    let mut solar = None;
    for (name, body) in [
        (
            "solar.md",
            "# Solar power\nPhotovoltaic panels convert sunlight into electrical energy.",
        ),
        (
            "bread.md",
            "# Baking bread\nMix flour with yeast and water. Knead the dough.",
        ),
        (
            "gardening.md",
            "# Gardening\nWater the roses and prune the hedges.",
        ),
        (
            "trains.md",
            "# Trains\nPassengers board the railway carriage at the station.",
        ),
    ] {
        let doc = device::import(
            &mut writer,
            &library.path().join("device-markdown-v1"),
            name.into(),
            body.into(),
        )?;
        if name == "solar.md" {
            solar = Some(doc.id);
        }
    }
    let writer = Arc::new(Mutex::new(writer));
    let runtime = EmbeddedRuntime::new(writer.clone(), library.path(), ServiceOptions::default())?;
    assert!(!runtime.device_search_status()?.enabled);
    runtime.set_device_semantic_enabled(true)?;
    let deadline = Instant::now() + Duration::from_secs(150);
    while runtime.device_search_status()?.embedded_documents != 4 {
        if Instant::now() >= deadline {
            return Err("embedded model/index exceeded acceptance deadline".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let query = "generate electricity from sunshine";
    let lexical = runtime.search_device(query, SearchMode::Exact)?;
    assert!(lexical.hits.is_empty());
    let semantic = runtime.search_device(query, SearchMode::Semantic)?;
    assert!(semantic.semantic_unavailable.is_none());
    assert_eq!(
        semantic.hits.first().map(|hit| &hit.document.id),
        solar.as_ref()
    );
    runtime.set_device_semantic_enabled(false)?;
    let disabled = runtime.search_device(query, SearchMode::Semantic)?;
    assert!(disabled.hits.is_empty() && disabled.semantic_unavailable.is_some());
    assert!(!runtime
        .search_device("Photovoltaic", SearchMode::Exact)?
        .hits
        .is_empty());
    println!(
        "{}",
        serde_json::json!({"documents":4,"embeddedDocuments":4,"dimensions":384,"lexicalHits":0,"topDocument":"solar.md","disabledSemanticPreservesTextSearch":true,"acceptsMobileDevice":false})
    );
    Ok(())
}

#[cfg(not(all(feature = "local-embed", unix)))]
fn main() {
    eprintln!("explicit embedded semantic probe requires local-embed and a Unix host");
    std::process::exit(1);
}
