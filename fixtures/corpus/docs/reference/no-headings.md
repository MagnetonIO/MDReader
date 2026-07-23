---
title: No Headings
tags: [reference, edge-case]
---

This file has frontmatter but not a single markdown heading. It is all body
text, the kind you get from an exported clipping or a plain paragraph note.

It still has structure of a sort: a couple of paragraphs, a list, and a link.

- one
- two
- three

The outline builder should produce an empty heading tree here and fall back to
the frontmatter `title`. Related reading: the
[deep headings](headings-deep.md) example.
