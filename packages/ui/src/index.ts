export { ReaderPlaceholder } from './reader-placeholder.js';

export { MarkdownReader } from './markdown-reader.js';
export type {
  MarkdownReaderHandle,
  MarkdownReaderProps,
  ScrollAnchor,
} from './markdown-reader.js';

export { Outline } from './outline.js';
export type { OutlineProps } from './outline.js';

export { SearchPalette } from './search/search-palette.js';
export type {
  SearchPaletteProps,
  PaletteMode,
  HitTarget,
  PaletteFile,
  SearchAdapter,
  SearchHit,
  SearchExplain,
  SearchOutcome,
  RelatedHit,
  SearchIntent,
} from './search/search-palette.js';
export {
  INTENTS,
  intentPlan,
  cycleIntent,
  sortHits,
  filterRelated,
  relatedHits,
  relatedAvailabilityMessage,
  relatedComponentsText,
  parseSnippet,
  buildBreadcrumb,
} from './search/model.js';
export type { RelatedOutcome, RelatedAvailability, IntentMeta, IntentPlan, Crumb, SnippetSegment } from './search/model.js';
export { fuzzyScore, rankFiles } from './search/fuzzy.js';
export type { FileMatch } from './search/fuzzy.js';

export { FrontmatterCard } from './components/frontmatter-card.js';
export { CodeBlock } from './components/code-block.js';
export { MermaidBlock } from './components/mermaid-block.js';
export { WikiLink } from './components/wiki-link.js';
export { MdLink } from './components/md-link.js';
export { MdImage } from './components/md-image.js';
export { ErrorBoundary } from './components/error-boundary.js';

export { ReaderProvider, useReaderContext } from './reader-context.js';
export type { NavigateEvent, ReaderContextValue } from './reader-context.js';

export {
  PlatformProvider,
  usePlatform,
  useOptionalPlatform,
} from './platform-provider.js';
export type { Platform } from './platform-provider.js';

export type { SearchCoverage, SearchCoverageSource } from './search/coverage.js';

export { fileContext } from './search/file-context.js';
