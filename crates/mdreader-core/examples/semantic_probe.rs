//! Explicit real-model acceptance; never part of deterministic/offline tests.
//! cargo run -p mdreader-core --features local-embed --example semantic_probe -- CACHE_DIRECTORY
#[cfg(feature = "local-embed")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use mdreader_core::{BgeEmbedder, ReaderStore, SourceRoot, EMBEDDING_DIM, MODEL_NAME};
    use std::{path::PathBuf, time::Instant};
    let cache = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("explicit public-model cache directory is required")?,
    );
    let directory = tempfile::tempdir()?;
    let notes = tempfile::tempdir()?;
    let corpus=[("solar.md","# Rooftop energy\nPhotovoltaic panels convert sunlight into electrical power for homes.\n"),("bread.md","# Kitchen recipe\nMix flour with yeast and water. Knead the dough, let it rise and bake a loaf.\n"),("garden.md","# Garden care\nWater tomato seedlings, fertilize the soil and remove weeds from vegetable beds.\n"),("trains.md","# Railway travel\nPassengers board scheduled trains at platforms and buy tickets for their journeys.\n")];
    for (path, body) in corpus {
        std::fs::write(notes.path().join(path), body)?;
    }
    let mut store = ReaderStore::open(directory.path())?;
    let source = store.add_local_source(
        "Synthetic model probe",
        SourceRoot::capture(notes.path().canonicalize()?)?,
    )?;
    store.index_source(&source.id)?;
    let opened = Instant::now();
    let mut model = BgeEmbedder::open(&cache)?;
    let model_open_ms = opened.elapsed().as_secs_f64() * 1000.;
    let mut vector_count = 0;
    for document in store.list_documents(&source.id)? {
        let (revision, texts) = store.embedding_inputs(&source.id, &document.path)?;
        let vectors = model.embed(&texts)?;
        for vector in &vectors {
            assert_eq!(vector.len(), EMBEDDING_DIM);
            let norm = vector
                .iter()
                .map(|x| f64::from(*x).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(
                (norm - 1.).abs() < 0.001,
                "model normalization contract changed"
            );
        }
        vector_count += vectors.len();
        store.install_embeddings(&source.id, &document.path, &revision, &vectors)?;
    }
    let query = "generate electricity from sunshine";
    let sources = vec![source.id];
    assert!(
        store.search(query, &sources, 10)?.is_empty(),
        "probe must require paraphrase retrieval"
    );
    let mut samples = Vec::new();
    let mut top = None;
    for _ in 0..20 {
        let started = Instant::now();
        let vector = model.embed(&[query.into()])?.remove(0);
        let hits = store.hybrid_search(query, &vector, &sources, 10)?;
        samples.push(started.elapsed().as_secs_f64() * 1000.);
        assert_eq!(hits.first().map(|hit| hit.path.as_str()), Some("solar.md"));
        assert!(hits[0].semantic_rank.is_some());
        assert!(hits[0].lexical_rank.is_none());
        top = Some(hits[0].clone());
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"kind":"actual-bge-paraphrase-probe","model":MODEL_NAME,"dimensions":EMBEDDING_DIM,"documents":corpus.len(),"vectors":vector_count,"model_open_ms":model_open_ms,"warm_hybrid_p95_ms":samples[18],"warm_hybrid_samples":samples,"query":query,"lexical_hits":0,"top_path":top.as_ref().map(|hit|&hit.path),"semantic_rank":top.as_ref().and_then(|hit|hit.semantic_rank),"accepts_10k_hybrid_budget":false,"accepts_mobile":false})
        )?
    );
    Ok(())
}
#[cfg(not(feature = "local-embed"))]
fn main() {
    eprintln!("enable local-embed for the explicit model acceptance probe");
    std::process::exit(2);
}
