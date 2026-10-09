//! Reciprocal-rank fusion after each retrieval arm has its final global order.
use crate::Evidence;
use std::collections::HashMap;

pub(crate) fn fuse(lexical: Vec<Evidence>, semantic: Vec<Evidence>, limit: usize) -> Vec<Evidence> {
    let mut merged: HashMap<(String, String, String, u32), Evidence> = HashMap::new();
    for (is_semantic, arm) in [(false, lexical), (true, semantic)] {
        for (rank, mut hit) in arm.into_iter().enumerate() {
            let rank = rank as u32 + 1;
            hit.lexical_rank = (!is_semantic).then_some(rank);
            hit.semantic_rank = is_semantic.then_some(rank);
            let key = (
                hit.source_id.clone(),
                hit.path.clone(),
                hit.revision.clone(),
                hit.line_start,
            );
            let score = 1.0 / (60.0 + f64::from(rank));
            if let Some(previous) = merged.get_mut(&key) {
                previous.score += score;
                if is_semantic {
                    previous.semantic_rank = Some(rank);
                } else {
                    previous.lexical_rank = Some(rank);
                }
            } else {
                hit.score = score;
                merged.insert(key, hit);
            }
        }
    }
    let mut hits: Vec<_> = merged.into_values().collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
            .then(a.line_start.cmp(&b.line_start))
            .then_with(|| a.source_id.cmp(&b.source_id))
    });
    hits.truncate(limit);
    hits
}
