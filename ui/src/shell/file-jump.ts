/**
 * Fuzzy file matching for the Cmd+P jump palette. A subsequence matcher with
 * boundary/contiguity bonuses — enough to feel good, small enough to read. Pure
 * and framework-free so it is unit-tested directly.
 */

import type { WorkspaceFile } from '../platform/fs.js';

const SEPARATORS = new Set(['/', '\\', '-', '_', '.', ' ']);

/**
 * Score `query` against `target` as a case-insensitive subsequence, or `null`
 * when `query` is not a subsequence. Higher is better: contiguous runs and
 * matches at word boundaries score more; leading gaps cost a little.
 */
export function fuzzyScore(query: string, target: string): number | null {
  if (query.length === 0) return 0;
  const q = query.toLowerCase();
  const t = target.toLowerCase();

  let score = 0;
  let qi = 0;
  let previousMatch = -2;
  for (let ti = 0; ti < t.length && qi < q.length; ti += 1) {
    if (t[ti] !== q[qi]) continue;

    let charScore = 1;
    if (ti === previousMatch + 1) charScore += 3; // contiguous run
    if (ti === 0 || SEPARATORS.has(t[ti - 1]!)) charScore += 4; // word boundary
    charScore -= Math.min(ti - previousMatch - 1, 3) * 0.2; // small gap penalty

    score += charScore;
    previousMatch = ti;
    qi += 1;
  }

  return qi === q.length ? score : null;
}

export interface FileMatch {
  file: WorkspaceFile;
  score: number;
}

/**
 * Rank `files` for `query`. Each file is scored against its name (weighted) and
 * its full path; the better wins. An empty query returns the files unranked,
 * capped to `limit`.
 */
export function rankFiles(files: WorkspaceFile[], query: string, limit = 100): FileMatch[] {
  const trimmed = query.trim();
  if (trimmed.length === 0) {
    return files.slice(0, limit).map((file) => ({ file, score: 0 }));
  }

  const matches: FileMatch[] = [];
  for (const file of files) {
    const nameScore = fuzzyScore(trimmed, file.name);
    const pathScore = fuzzyScore(trimmed, file.path);
    // Name hits are worth more than the same subsequence buried in a path.
    const best = Math.max(
      nameScore === null ? -Infinity : nameScore + 6,
      pathScore === null ? -Infinity : pathScore,
    );
    if (best > -Infinity) matches.push({ file, score: best });
  }

  matches.sort((a, b) => b.score - a.score || a.file.name.localeCompare(b.file.name));
  return matches.slice(0, limit);
}

/** Convenience: the ranked files only. */
export function filterFiles(files: WorkspaceFile[], query: string, limit = 100): WorkspaceFile[] {
  return rankFiles(files, query, limit).map((match) => match.file);
}
