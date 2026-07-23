---
title: Unicode, Emoji, CJK
tags: [edge-case, unicode]
---

# Unicode 🌍 表意文字 テスト

Mixed scripts in one heading and one body.

## Emoji

Status: ✅ done, 🚧 in progress, ❌ blocked. Family: 👩‍👩‍👧‍👦 (a ZWJ sequence that
is one grapheme but several code points). Flags: 🇯🇵 🇰🇷 🇺🇸. Skin tone: 👍🏽.

## CJK

- 中文：知识库把文档变成向量。
- 日本語：ドキュメントをベクトルに変換します。
- 한국어: 문서를 벡터로 변환합니다.

## Diacritics and RTL

Café, naïve, Zürich, Håkon, Þorvaldur. Arabic: مرحبا بالعالم. Hebrew: שלום עולם.

## Widths and combining marks

Full-width ＡＢＣ next to ASCII ABC. Combining: e + ́ = é. Zalgo-lite: a̐b̽c̈.

Character counts here are a good test for byte-vs-grapheme offset handling in
[provenance](../notes/zettel/node-provenance.md).
