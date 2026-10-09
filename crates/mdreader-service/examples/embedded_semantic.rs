//! Explicit real-model owner acceptance; not part of ordinary unit tests.
//! Usage: cargo run -p mdreader-service --features local-embed --example
//! embedded_semantic -- /ABS/PUBLIC_BGE_CACHE
#[cfg(all(feature = "local-embed", unix))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use mdreader_core::{ReaderStore, SourceRoot};
    use mdreader_service::{EmbeddedRuntime, Request, SearchMode, ServiceOptions};
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
    let notes = tempfile::tempdir()?;
    for (path, body) in [
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
        std::fs::write(notes.path().join(path), body)?;
    }
    // Reuse only the explicitly selected public model bytes, never a user library.
    std::os::unix::fs::symlink(cache, library.path().join("models"))?;
    let mut writer = ReaderStore::open(library.path())?;
    let source = writer
        .add_local_source(
            "Public fixture",
            SourceRoot::capture(notes.path().canonicalize()?)?,
        )?
        .id;
    let writer = Arc::new(Mutex::new(writer));
    let runtime = EmbeddedRuntime::new(writer.clone(), library.path(), ServiceOptions::default())?;
    runtime.execute(Request::StartIndex {
        source_id: source.clone(),
    })?;
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let job = runtime.execute(Request::JobStatus {
            source_id: source.clone(),
        })?;
        match job["phase"].as_str() {
            Some("done") => break,
            Some("model-unavailable" | "error") => return Err("embedded model/index failed".into()),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err("embedded indexing deadline exceeded".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let query = "generate electricity from sunshine";
    let lexical = runtime.execute(Request::Search {
        query: query.into(),
        sources: vec![source.clone()],
        limit: 10,
    })?;
    if !lexical.as_array().is_some_and(Vec::is_empty) {
        return Err("fixture overlaps lexical query".into());
    }
    let outcome = runtime.execute(Request::SearchRanked {
        query: query.into(),
        sources: vec![source.clone()],
        limit: 10,
        mode: SearchMode::Semantic,
    })?;
    if !outcome["semantic_unavailable"].is_null() || outcome["hits"][0]["path"] != "solar.md" {
        return Err("real semantic retrieval failed".into());
    }
    let vectors = writer
        .lock()
        .map_err(|_| "writer failed")?
        .index_status(&[source])?
        .embedded;
    if vectors != 4 {
        return Err("fixture vector count differs".into());
    }
    println!(
        "{}",
        serde_json::json!({
            "kind":"embedded-owner-real-bge", "model":"bge-small-en-v1.5", "dimensions":384,
            "documents":4, "vectors":vectors, "lexicalHits":0, "topPath":"solar.md",
            "desktopCaptureEnabled":false, "acceptsMobileDevice":false,
        })
    );
    Ok(())
}
#[cfg(not(all(feature = "local-embed", unix)))]
fn main() {
    eprintln!("explicit embedded semantic probe requires local-embed and a Unix host");
    std::process::exit(1);
}
