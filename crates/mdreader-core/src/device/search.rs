//! Device retrieval spans bounded source groups, with one query vector and
//! global arm ordering before fusion. Every returned hit matches a live receipt.
use super::{DeviceDocument, DeviceHit};
use crate::{Evidence, ReaderStore, Result};

pub fn ranked_search(
    store: &ReaderStore,
    documents: &[DeviceDocument],
    query: &str,
    vector: Option<&[f32]>,
    semantic_only: bool,
) -> Result<Vec<DeviceHit>> {
    if query.len() > 1024 || documents.len() > 1000 {
        return Err(crate::ReaderError::Invalid(
            "Invalid device search bounds".into(),
        ));
    }
    if query.trim().is_empty() || documents.is_empty() {
        return Ok(vec![]);
    }
    let sources: Vec<_> = documents.iter().map(|doc| doc.source_id.clone()).collect();
    let mut lexical = vec![];
    let mut semantic = vec![];
    for selected in sources.chunks(64) {
        if !semantic_only {
            lexical.extend(store.search(query, selected, 100)?);
        }
        if let Some(vector) = vector {
            semantic.extend(store.semantic_search_query(query, vector, selected, 100)?);
        }
    }
    let order = |arm: &mut Vec<Evidence>| {
        arm.retain(|hit| {
            documents.iter().any(|doc| {
                doc.source_id == hit.source_id
                    && doc.revision == hit.revision
                    && hit.path == "document.md"
            })
        });
        arm.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.source_id.cmp(&b.source_id))
                .then(a.line_start.cmp(&b.line_start))
        });
        arm.truncate(100);
    };
    order(&mut lexical);
    order(&mut semantic);
    let hits = if vector.is_some() && !semantic_only {
        crate::ranking::fuse(lexical, semantic, 50)
    } else if semantic_only {
        semantic
    } else {
        lexical
    };
    Ok(hits
        .into_iter()
        .take(50)
        .filter_map(|hit| {
            documents
                .iter()
                .find(|doc| doc.source_id == hit.source_id && doc.revision == hit.revision)
                .map(|document| DeviceHit {
                    document: document.clone(),
                    line: hit.line_start,
                    text: hit.text,
                })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn semantic_hits_span_source_groups_and_respect_exact_receipts() {
        let library = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        let mut store = ReaderStore::open(library.path()).unwrap();
        let mut docs = vec![];
        for index in 0..65 {
            let doc = crate::device::import(
                &mut store,
                files.path(),
                format!("note-{index}.md"),
                format!("# Note {index}\nPublic fixture passage."),
            )
            .unwrap();
            store.index_source(&doc.source_id).unwrap();
            let mut vector = vec![0.0; 384];
            vector[usize::from(index != 64)] = 1.0;
            store
                .install_embeddings(&doc.source_id, "document.md", &doc.revision, &[vector])
                .unwrap();
            docs.push(doc);
        }
        let mut query = vec![0.0; 384];
        query[0] = 1.0;
        let hits = ranked_search(&store, &docs, "unmatchedparaphrase", Some(&query), true).unwrap();
        assert_eq!(hits[0].document.id, docs[64].id);
        assert!(
            ranked_search(&store, &docs, "unmatchedparaphrase", None, false)
                .unwrap()
                .is_empty()
        );
        let mut stale = docs.clone();
        stale[64].revision = "stale".into();
        assert!(
            ranked_search(&store, &stale, "unmatchedparaphrase", Some(&query), true)
                .unwrap()
                .iter()
                .all(|hit| hit.document.id != docs[64].id)
        );
        store
            .set_source_enabled(&docs[64].source_id, false)
            .unwrap();
        assert!(ranked_search(&store, &docs, "unmatchedparaphrase", Some(&query), true).is_err());
    }
}
