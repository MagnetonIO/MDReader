import { useEffect, useRef, useState } from 'react';
import { NativeProviderError, ProviderRequestRetiredError } from '../../platform/providers';
import type { NativeProvidersBackend, ProviderCreateIntent, ProviderCreateResult, ProviderReaderReference, ProviderSource } from '../../platform/providers';
import { catalogReference } from '../catalog/backend';
import { canCreateCloudDocument, cloudMarkdownFilename } from './cloud-create-model';

type Backend = Pick<NativeProvidersBackend, 'createDocument' | 'inspectCreate'>;
type Attempt = { intent: ProviderCreateIntent; state: ProviderCreateResult['state']; error: string | null; busy: boolean };
export function useCloudCreation(backend: Backend, scope: string | number, onCreated?: (reference: ProviderReaderReference) => void, beforeCreate?: () => Promise<void>) {
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const attempts = useRef(new Map<string, Attempt>());
  const [source, setSource] = useState<ProviderSource | null>(null);
  const [filename, setFilename] = useState('Untitled.md');
  const [, render] = useState(0);
  const current = useRef({ scope, source, onCreated, backend });
  current.current = { scope, source, onCreated, backend };
  const attempt = source ? attempts.current.get(source.sourceId) : undefined;
  function open(target: ProviderSource) {
    if (!canCreateCloudDocument(target) || !backend.createDocument || !backend.inspectCreate) return;
    const retained = attempts.current.get(target.sourceId);
    setFilename(retained?.intent.filename ?? 'Untitled.md');
    setSource(target);
  }
  async function run(inspect: boolean) {
    const selected = current.current.source;
    if (!selected) return;
    let value = attempts.current.get(selected.sourceId);
    if (value?.busy || !canCreateCloudDocument(selected)) return;
    const name = cloudMarkdownFilename(filename);
    if (!inspect) {
      if (!name || value && !['absent', 'conflict'].includes(value.state)) return;
      if (!value || value.state === 'conflict' || value.intent.filename !== name) {
        if (attempts.current.size >= 32 && !attempts.current.has(selected.sourceId)) return;
        value = { intent: { sourceId: selected.sourceId, operationId: crypto.randomUUID(), filename: name, markdown: '' }, state: 'prepared', busy: false, error: null };
        attempts.current.set(selected.sourceId, value);
      }
    }
    if (!value) return;
    const retained = value, openedScope = current.current.scope;
    const stillCurrent = () => mounted.current && current.current.backend === backend && current.current.scope === openedScope;
    retained.busy = true; retained.error = null; render(n => n + 1);
    try {
      if (!inspect && beforeCreate) await beforeCreate();
      if (!inspect && !stillCurrent()) {
        retained.state = 'absent';
        retained.error = 'The connection changed before creation. Try again from the current connection.';
        return;
      }
      const result = inspect ? await backend.inspectCreate!(selected.sourceId, retained.intent.operationId)
        : await backend.createDocument!(retained.intent, stillCurrent);
      if (result.operation !== (inspect ? 'inspect_create' : 'create_document') || result.operationId !== retained.intent.operationId
        || !['absent', 'prepared', 'dispatched', 'unknown', 'accepted', 'published', 'conflict'].includes(result.state)) throw new Error('The creation result could not be confirmed.');
      if (result.state === 'published') {
        const projection = result.projection;
        if (!projection || !projection.readerSourceId.startsWith('provider:')
          || selected.readerSourceId && projection.readerSourceId !== selected.readerSourceId) throw new Error('The created document could not be verified.');
        const reference = catalogReference({ kind: 'exact', sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision }, retained.intent.filename);
        retained.state = 'published';
        if (mounted.current && current.current.backend === backend && current.current.scope === openedScope && current.current.source === selected) {
          attempts.current.delete(selected.sourceId); setSource(null); current.current.onCreated?.(reference);
        }
      } else {
        if (result.projection) throw new Error('The creation result could not be confirmed.');
        retained.state = result.state;
        if (result.state === 'conflict') retained.error = 'This filename is unavailable. Choose another name.';
      }
    } catch (cause) {
      // Native may identify an earlier durable same-name attempt after app restart.
      const prior = cause instanceof NativeProviderError && cause.detail.kind === 'unknown_write' ? cause.detail.operationId : null;
      if (prior && /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i.test(prior)) retained.intent = { ...retained.intent, operationId: prior };
      if (cause instanceof ProviderRequestRetiredError) {
        retained.state = 'absent'; retained.error = cause.message;
      } else if (!inspect && cause instanceof NativeProviderError && cause.detail.kind === 'invalid') {
        retained.state = 'absent';
        retained.error = 'Choose a valid Markdown filename.';
      } else {
        retained.state = 'unknown';
        retained.error = 'Creation could not be confirmed. Check status before trying again.';
      }
    } finally { retained.busy = false; if (mounted.current) render(n => n + 1); }
  }
  return { source, filename, open, close: () => setSource(null), setFilename,
    busy: attempt?.busy ?? false, pending: Boolean(attempt && !['absent', 'conflict'].includes(attempt.state)),
    retry: attempt?.state === 'absent', error: attempt?.error ?? null,
    create: () => void run(false), inspect: () => void run(true) };
}
