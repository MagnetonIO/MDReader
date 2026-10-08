export { parseMarkdown } from './pipeline.js';
// NOTE: the worker helpers (`parseMarkdownAuto`, `parseMarkdownInWorker`,
// `createMarkdownWorker`) are intentionally NOT re-exported here. They live only
// on the `@mdreader/markdown/worker` subpath. `createMarkdownWorker` contains the
// literal `new Worker(new URL('./worker.js', import.meta.url))` that bundlers
// must statically detect; re-exporting it from the package root would drag that
// literal into every consumer of the root — including apps that never use the
// worker (Next.js/Turbopack stalls building a server bundle that contains it).
export {
  WORKER_BYTE_THRESHOLD,
  VIRTUALIZATION_BLOCK_THRESHOLD,
  shouldVirtualize,
  shouldUseWorker,
} from './thresholds.js';
export { isFrontmatterError } from './types.js';
export { readerSanitizeSchema, USER_CONTENT_PREFIX } from './sanitize-schema.js';

export type {
  ParsedDoc,
  ParseOptions,
  FrontmatterData,
  FrontmatterError,
  OutlineItem,
  DocStats,
} from './types.js';
export type { WikiLink } from './wikilink.js';
export type { WorkerRequest, WorkerResponse } from './worker.js';
