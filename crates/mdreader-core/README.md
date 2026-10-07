# MDReader core

This crate owns Markdown parsing, retained local filesystem authority, and the new reader SQLite storage API. It has no CatDB dependency. The existing ingest layer reexports parsing/scanning from here while its legacy catalog integration is migrated.

`library.sqlite` holds durable source grants and bookmarks. `index.sqlite` holds derived documents, headings, links, FTS5 and 384-dimensional sqlite-vec embeddings. Rebuilding or deleting the index preserves user state and source content. Reads use the admitted filesystem object and return a content revision; an expected revision refuses stale evidence. Every search requires explicit enabled source scope.

The optional `local-embed` feature uses the existing fastembed 5.17.3 BGE-small-en-v1.5 contract. Lexical indexing does not load a model. Tests use explicit vector fixtures to validate SQLite ranking, not to claim language-model retrieval quality.

Run `cargo test -p mdreader-core` and `cargo check -p mdreader-core --features local-embed`. Platform compilation checks and current integration boundaries are recorded in the implementation ledger. Captures, cloud sync and the GUI storage cutover are subsequent implementation slices.
