/**
 * The lens band: one line across the top of the reading column naming the
 * interpretation currently in force.
 *
 * This is the thesis where it can actually be seen. The prose below the band
 * does not change when a lens is chosen — the same bytes, the same document,
 * untouched. What changes is only which typed reading of the corpus the app is
 * offering, and the band says so in the app's own voice, every time, rather
 * than leaving it to a settings screen nobody reads.
 *
 * It renders only while a lens is in force. The plain reading gets no banner,
 * because the plain reading is the product.
 */

import type { SchemaSummary } from '../../platform/semantics.js';
import { UNVERSIONED_REASON, checksumLabel, plural, versionLabel } from './lens-format.js';

export interface LensBandProps {
  /** The schema in force. */
  schema: SchemaSummary;
  /** Open the Semantic view sheet on this schema. */
  onOpen: () => void;
  /** Return to the plain reading. */
  onClear: () => void;
}

export function LensBand({ schema, onOpen, onClear }: LensBandProps) {
  const unversioned = schema.version === null;
  return (
    <div className="lens-band" aria-label="Active lens">
      <span className="lens-band__mark" aria-hidden="true" />
      <button type="button" className="lens-band__schema lens-mono" onClick={onOpen} title="Open this lens">
        {schema.name}
      </button>
      <span className="lens-band__counts">
        {schema.objectCount.toLocaleString()} in the graph
        {/* "claims", not "claims awaiting review": the proposal store counts
            claims in ANY review state, and calling them all pending would
            misreport how much is left to rule on. */}
        {schema.approvalRequired ? ` · ${plural(schema.proposalCount, 'claim')}` : ''}
        {` · ${plural(schema.equationCount, 'law')}`}
      </span>
      <span
        className={`lens-band__version lens-mono${unversioned ? ' lens-band__version--absent' : ''}`}
        title={unversioned ? UNVERSIONED_REASON : checksumLabel(schema.checksum)}
      >
        {versionLabel(schema.version)}
      </span>
      <span className="lens-band__spacer" />
      {/* Stated, not implied. The lens is a reading; the files are the files. */}
      <span className="lens-band__untouched">Sources untouched</span>
      <button type="button" className="link-btn" onClick={onClear}>
        Plain reading
      </button>
    </div>
  );
}
