import { coverageRefreshLabel, indexObservation, semanticModelLabel, type SearchCoverage, type SearchCoverageSource } from './coverage.js';

const labels: Record<SearchCoverageSource['kind'], string> = {
  local: 'Open folder', capture: 'Captured notes', device: 'On this device', provider: 'Cloud source',
};
const number = (value: number) => value.toLocaleString();

/** Counts describe the owner's retained index observation for the final selected
 * scope, never the file list or a promise of a shared transaction with the hits.
 * Opaque authority/index markers have independent domains and are not compared. */
export function SearchCoveragePanel({ coverage }: { coverage: SearchCoverage }) {
  const documents = coverage.sources.reduce((sum, source) => sum + source.indexedDocuments, 0);
  const chunks = coverage.sources.reduce((sum, source) => sum + source.indexedChunks, 0);
  const embedded = coverage.sources.reduce((sum, source) => sum + source.embeddedChunks, 0);
  const censusKnown = coverage.sources.length > 0 && coverage.sources.every(source => source.eligibleDocuments !== null);
  const eligible = censusKnown ? coverage.sources.reduce((sum, source) => sum + (source.eligibleDocuments ?? 0), 0) : null;
  const refresh = coverageRefreshLabel(coverage);
  const scope = [...new Set(coverage.sources.map(source => labels[source.kind]))].join(' · ');
  return <details className="mdsearch-coverage" aria-label="Content index coverage">
    <summary><span>{eligible === null ? `${number(documents)} Markdown documents indexed · eligible total unknown`
      : `${number(documents)} of ${number(eligible)} eligible Markdown documents indexed`}{refresh ? ` · ${refresh}` : null}</span><span className="mdsearch-coverage__scope">Content scope: {scope || 'No selected sources'}</span></summary>
    <div className="mdsearch-coverage__body">
      <p>{number(chunks)} indexed content sections. {semanticModelLabel(coverage.model)}; {number(embedded)} of {number(chunks)} sections embedded.</p>
      {coverage.sources.length === 0 ? <p>No searchable sources in the selected scope.</p> : <ul>
        {coverage.sources.map((source, index) => <li key={source.sourceId}>
          <strong>{labels[source.kind]}{coverage.sources.filter(other => other.kind === source.kind).length > 1 ? ` ${index + 1}` : ''}</strong>
          <p>{number(source.indexedDocuments)} Markdown documents indexed · {number(source.indexedChunks)} content sections · {number(source.embeddedChunks)} embedded.
            {source.eligibleDocuments === null ? ' Eligible Markdown total unknown.' : ` Eligible Markdown total: ${number(source.eligibleDocuments)}.`}</p>
          <p>{indexObservation(source)} {source.observedAtMs === null ? 'Observation time unknown.' : `Observed ${new Date(source.observedAtMs).toLocaleString()}.`}</p>
          {source.kind === 'provider' && source.metadata ? <p>Cloud filenames: {number(source.metadata.knownDocuments)} ({source.metadata.state === 'complete_selected' ? 'selected metadata complete' : source.metadata.state === 'partial' ? 'partial metadata' : 'not yet observed'}).
            {source.admittedBodies !== null ? ` Document bodies recorded: ${number(source.admittedBodies)}.` : ''} Filenames are not searchable document text.</p> : null}
        </li>)}
      </ul>}
    </div>
  </details>;
}
