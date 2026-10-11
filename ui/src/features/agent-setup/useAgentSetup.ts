import { useEffect, useMemo, useRef, useState } from 'react';
import { simpleAgentBackend, type AgentSetupSelection, type AgentSetupState, type SimpleAgentBackend } from '../../platform/agent-setup';

/** Every response belongs to one exact agent/configuration target; no automatic installs or grants. */
export function useAgentSetup(root: string | null, active: boolean, backend: SimpleAgentBackend = simpleAgentBackend) {
  const [agent, setAgent] = useState<AgentSetupSelection['agent']>('codex');
  const [scope, setScope] = useState<AgentSetupSelection['scope']>('user');
  const [project, setProject] = useState(root ?? '');
  const selection = useMemo<AgentSetupSelection>(() => ({ agent, scope, project_root: scope === 'project' ? project || null : null }), [agent, scope, project]);
  const identity = useMemo(() => ({}), [selection, active, backend]);
  const latest = useRef(identity); latest.current = identity;
  const sequence = useRef(0);
  const [result, setResult] = useState<{ identity: object; state: AgentSetupState } | null>(null);
  const [operation, setOperation] = useState<{ identity: object; busy: boolean; error: string | null; notice: string | null } | null>(null);
  const [sources, setSources] = useState<{ identity: object; ids: string[] } | null>(null);
  const state = result?.identity === identity ? result.state : null;
  const ids = sources?.identity === identity ? sources.ids : state?.source_ids ?? [];
  const busy = operation?.identity === identity && operation.busy;
  async function run(task: () => Promise<AgentSetupState>, notice: string | null = null) {
    if (!active || latest.current !== identity) return;
    const generation = ++sequence.current, target = identity;
    setOperation({ identity: target, busy: true, error: null, notice: null });
    try {
      const value = await task();
      if (latest.current !== target || sequence.current !== generation) return;
      setResult({ identity: target, state: value });
      setSources({ identity: target, ids: value.source_ids.filter(id => value.sources.some(source => source.id === id)) });
      setOperation({ identity: target, busy: false, error: null, notice });
    } catch (cause) {
      if (latest.current === target && sequence.current === generation)
        setOperation({ identity: target, busy: false, error: cause instanceof Error ? cause.message : 'Agent setup could not be completed.', notice: null });
    }
  }
  useEffect(() => {
    if (active) void run(() => backend.state(selection));
    return () => { sequence.current += 1; };
  }, [identity]);
  return {
    agent, scope, project, setAgent, setScope, setProject, selection, state, ids, busy,
    error: operation?.identity === identity ? operation.error : null,
    notice: operation?.identity === identity ? operation.notice : null,
    selectSource(id: string, checked: boolean) {
      setSources({ identity, ids: checked ? [...new Set([...ids, id])] : ids.filter(value => value !== id) });
    },
    refresh: () => run(() => backend.state(selection)),
    install: () => run(() => backend.install(selection)),
    enable: () => run(() => backend.setMcp(selection, ids, true)),
    disable: () => run(() => backend.setMcp(selection, [], false), 'MCP configuration disabled.'),
    uninstall: () => run(() => backend.uninstall(selection), 'MDReader’s owned integration entries were removed.'),
  };
}
