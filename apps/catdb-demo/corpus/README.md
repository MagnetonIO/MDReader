# CatDB demo workspace

This small library is the immutable source corpus for the hosted MDReader
viewer. CatDB indexes these Markdown files during the container build. Search
and document reads run against that prebuilt workspace; the public runtime
does not expose indexing or write operations.

## Start here

Read [Schema-first applications](guides/schema-first.md), then follow the
mapping into [Provenance](notes/provenance.md).
