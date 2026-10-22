import type { ProviderSource } from '../../platform/providers';
import { ProviderIcon } from './ProviderIcon';
import { checkedDate, providerName, sourceDetails, sourceStatus, sourceStatusLabel } from './source-presentation';

/** Two compact lines: identity with status, then the saved selection and file-list observation. */
export function ProviderSourceSummary({ source, allowEditing }: { source: ProviderSource; allowEditing: boolean }) {
  const status = sourceStatus(source);
  const checked = checkedDate(source.metadataListing?.observedAtMs);
  return <div className="provider-source__identity">
    <ProviderIcon provider={source.kind} className="provider-source__icon" />
    <div className="provider-source__text">
      <div className="provider-source__title">
        <h3>{source.name}</h3>
        <span className="provider-source__kind">{providerName(source.kind)}</span>
        <span className={`provider-status provider-status--${status}`}>{sourceStatusLabel[status]}</span>
      </div>
      <p className="provider-source__details" title={checked ? `Last checked ${checked.toLocaleString()}` : undefined}>
        {sourceDetails(source, allowEditing).join(' · ')}
      </p>
    </div>
  </div>;
}
