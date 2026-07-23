---
title: KaTeX Block Math
tags: [guide, math]
---

# KaTeX Block Math

Softmax over logits:

$$
\sigma(z)_i = \frac{e^{z_i}}{\sum_{j=1}^{K} e^{z_j}}
$$

An aligned derivation:

$$
\begin{aligned}
\lVert a - b \rVert^2 &= (a - b) \cdot (a - b) \\
                      &= \lVert a \rVert^2 - 2\,a \cdot b + \lVert b \rVert^2
\end{aligned}
$$

A matrix:

$$
M = \begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}
$$

Inline math still works here too: $e^{i\pi} + 1 = 0$. See also
[inline math](katex-inline.md).
