import { useEffect, useRef, useState } from 'react';
import { isTauri } from '../../shell/persistence';
import type { CanonicalReaderReference } from './contracts';
import { resolveDocumentLocation, type DocumentKind, type DocumentLocation, type LocationResolver } from './document-location';

const nativeLocation: LocationResolver = (reference, kind, current) => isTauri()
  ? resolveDocumentLocation(reference, kind, current) : Promise.resolve(null);

export function DocumentDetails({ reference, kind, verified, resolver = nativeLocation }: {
  reference: CanonicalReaderReference; kind: DocumentKind; verified: boolean; resolver?: LocationResolver;
}) {
  const [resolved, setResolved] = useState<{ reference: CanonicalReaderReference; resolver: LocationResolver; kind: DocumentKind; value: DocumentLocation } | null>(null);
  const selected = useRef({ reference, resolver, kind, verified }); selected.current = { reference, resolver, kind, verified };
  useEffect(() => {
    let active = true;
    const current = () => active && selected.current.reference === reference && selected.current.resolver === resolver
      && selected.current.kind === kind && selected.current.verified;
    if (verified) void resolver(reference, kind, current).then(value => {
      if (value && current()) setResolved({ reference, resolver, kind, value });
    }).catch(() => { /* Unknown location is omitted; receipts never become invented paths. */ });
    return () => { active = false; };
  }, [reference, kind, resolver, verified]);
  const location = verified && resolved?.reference === reference && resolved.resolver === resolver && resolved.kind === kind ? resolved.value : null;
  const file = location?.file || (kind === 'local' ? reference.path.split('/').at(-1) : reference.title !== reference.path ? reference.title : null) || 'Markdown document';
  const source = location?.source || (kind === 'local' ? 'Local folder' : kind === 'provider' ? 'Cloud connection' : kind === 'capture' ? 'Captured output' : 'On this device');
  return <div className="document-details">
    <dl><dt>File</dt><dd>{file}</dd><dt>Source</dt><dd>{source}</dd>
      {location?.path ? <><dt>Full path</dt><dd>{location.path}</dd></> : null}
      {location?.location ? <><dt>{location.locationLabel || 'Location'}</dt><dd>{location.location}</dd></> : null}
    </dl>
    <details><summary>Technical details</summary>
      <dl><dt>Source ID</dt><dd>{reference.sourceId}</dd><dt>Revision</dt><dd>{reference.revision}</dd></dl>
    </details>
  </div>;
}
