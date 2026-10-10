# Content search observations

Filename lookup and indexed body search are separate operations. A filename
listing can include Markdown that has not been indexed, or ignored files which
the text index deliberately excludes. `fromIndex` identifies listing origin;
it does not measure content coverage.

The native content-search result carries an optional `SearchCoverageDto` for
the exact selected authorized source set. A superseded workspace result carries
no coverage. Each source reports indexed documents, passages, embedded passages,
an opaque authority marker, an independent index marker and its observation
time, and the current indexing job phase/generation. Never compare authority
and index tokens: they belong to different domains.

Counts describe the retained index observation, not a live filesystem census.
Local indexing accepts bounded UTF-8 `.md` and `.markdown`, honors ignore rules
and excludes hidden files and links/reparse points. A queued/running/failed
refresh can retain the previous successful index. Failure is a source job state;
there is no authoritative per-file failure or pending count. The eligible total
stays unknown for folders, captures and cloud sources. Do not show an invented
percentage or infer completeness from equal legacy progress counters.

Exact native device receipts narrow counts to their current document revision.
Their known denominator is one. Empty Markdown can be indexed with no passages.
Provider counts include only enabled current binding/revision projections;
metadata-only rows and cached bodies without admission are not text index rows.
The optional provider metadata census distinguishes never observed, partial and
completed selected metadata; admitted bodies are reported separately. Neither
count establishes that every remote Markdown body was fetched. Observing
coverage never fetches bodies, creates grants or schedules indexing/model work.

Model readiness is independent of lexical readiness and vector coverage.
`ready` does not mean every indexed passage has an embedding. Compare embedded
passages to indexed passages to describe partial vector coverage, and preserve
the search result's separate semantic-unavailable reason for requested ranking.

Coverage uses bounded SQLite observations and retained source verification.
Index/grant/job/projection changes across the observation refuse the result;
exact device receipts and metadata observations are checked again. The final
native boundary also checks the current workspace and reader-owned membership.
The DTO contains no private roots, provider keys, credentials or job error text.
