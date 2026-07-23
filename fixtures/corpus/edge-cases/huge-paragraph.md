---
title: Huge Paragraph
tags: [edge-case, layout]
---

# One Enormous Paragraph

The following is a single paragraph with no internal line breaks, the kind of thing you get when a note is dictated in one breath and never edited, and it keeps going and going without any punctuation-driven pause long enough to become its own block, which is exactly the case that trips up naive wrappers and offset trackers because there is no newline to key on until the very end, and it mentions CatDB and MDReader and ingest and storage and embeddings and provenance and the query engine and the planner and the graph view and search all in a row so that a tokenizer sees a dense run of domain terms, and it deliberately runs past the width any editor would soft-wrap at so that anything relying on hard line breaks to find sentence boundaries has to fall back on real sentence segmentation instead, and it even folds in a couple of inline bits like `code` and a [link](../index.md) and *emphasis* to make sure inline parsing still works inside a very long flow, and then, finally, after far too long, it stops.
