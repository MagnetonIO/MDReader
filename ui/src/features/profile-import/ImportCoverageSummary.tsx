import type { ImportCoverage, ImportReply } from './backend';

/** Native archive coverage describes preservation, never renewed source authority. */
export function ImportCoverageSummary({ origin, coverage, prepared }: {
  origin: ImportReply['origin'];
  coverage: ImportCoverage;
  prepared: boolean;
}) {
  const items: [string, number][] = [
    ['Documents and insights', coverage.documents],
    ['Document relationships', coverage.relationships],
    ['Archived reviews', coverage.reviews],
    ['Reading history entries', coverage.historyEntries],
  ];
  return <details className="profile-import__coverage">
    <summary>{prepared ? 'What was preserved' : 'What this library contains'}</summary>
    <dl>
      {items.map(([label, count]) => <div key={label}><dt>{label}</dt><dd>{count.toLocaleString()}</dd></div>)}
      <div><dt>Appearance and reader preferences</dt><dd>{coverage.settings ? 'Included' : 'Not found'}</dd></div>
    </dl>
    {origin === 'legacy_catdb' ? <p>Selected content becomes independent Markdown copies. Original history and reviews stay in the verified archive; old reading positions are not applied to a different version.</p> : null}
    {coverage.unavailableHistoryEntries ? <p>{coverage.unavailableHistoryEntries.toLocaleString()} historical reference{coverage.unavailableHistoryEntries === 1 ? '' : 's'} cannot be opened from the available content. Their records are kept, with no substitute document or automatic account connection.</p> : null}
    {coverage.unknownRecords ? <p>{coverage.unknownRecords.toLocaleString()} other record{coverage.unknownRecords === 1 ? ' is' : 's are'} preserved in the archive. They are not presented as converted documents.</p> : null}
    <p>The original library stays unchanged. Account credentials and folder permissions are not activated by importing.</p>
  </details>;
}
