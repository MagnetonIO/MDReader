---
title: Provenance
kind: note
---

# Provenance

Every result should retain enough evidence to explain which source supplied a
field and which schema and mapping versions interpreted it.

## Why it matters

Typed output without provenance can still be wrong in ways that look
plausible. Provenance makes the execution inspectable by an application, an
agent, or a human reviewer.

## Version identity

A deployed demo is pinned to one build commit, one catalog revision, and one
immutable corpus fingerprint. If any source byte changes, it is a different
deployment identity.
