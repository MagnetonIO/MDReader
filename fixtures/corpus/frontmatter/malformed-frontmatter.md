---
title: Malformed
tags: [unclosed, list
author "no colon here"
  bad_indent: : ::
nested:
    - a
  - b
---

# Malformed Frontmatter

The YAML above is deliberately broken: an unclosed flow sequence, a key with no
colon, doubled colons, and inconsistent indentation. A robust ingest should
recover — surface a metadata-parse warning, keep `title` if it can, and still
render the body — rather than dropping the whole document.

Compare with [typical](typical.md).
