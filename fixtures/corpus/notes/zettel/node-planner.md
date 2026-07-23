---
title: Planner
tags: [zettel]
---

# Planner

The planner rewrites a query into an execution plan: which indexes to hit, in
what order, and how to merge partial results. It is the bridge between raw
intent and [[node-query-engine]] execution.

It knows the cost of a trigram scan versus an ANN probe, so it can pick the
cheaper leg first. Feeds [[node-search]]. Home: [[graph-hub]].
