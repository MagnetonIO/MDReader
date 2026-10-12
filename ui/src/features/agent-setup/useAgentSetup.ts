import { agentSetupError } from './setup-error';
import { useEffect, useMemo, useRef, useState } from 'react';
import { simpleAgentBackend, type AgentSetupSelection, type AgentSetupState, type SimpleAgentBackend } from '../../platform/agent-setup';

/** Every response belongs to one exact agent/configuration target; no automatic installs or grants. */
export function useAgentSetup(root: string | null, active: boolean, backend: SimpleAgentBackend = simpleAgentBackend, initialAgent: AgentSetupSelection['agent'] = 'codex') {
  const [agent, setAgent] = useState<AgentSetupSelection['agent']>(initialAgent);
  const [scope, setScope] = useState<AgentSetupSelection['scope']>('user');
  const [project, setProject] = useState(root ?? '');
  const selection = useMemo<AgentSetupSelection>(() => ({ agent, scope, project_root: scope === 'project' ? project || null : null }), [agent, scope, project]);
  const identity = useMemo(() => ({}), [selection, active, backend]);
  const latest = useRef<object | null>(identity); latest.current = identity;
  const sequence = useRef(0);
  const activeTask = useRef<{ target: object; generation: number } | null>(null);
  const [result, setResult] = useState<{ identity: object; state: AgentSetupState } | null>(null);
  const [operation, setOperation] = useState<{ identity: object; busy: boolean; error: string | null; notice: string | null } | null>(null);
  const [sources, setSources] = useState<{ identity: object; ids: string[]; projects: string[]; edits: string[] } | null>(null);
  const state = result?.identity === identity ? result.state : null;
  const ids = sources?.identity === identity ? sources.ids : state?.source_ids ?? [];
  const projectIds = sources?.identity === identity ? sources.projects : state?.reporting_project_ids ?? [];
  const editIds = sources?.identity === identity ? sources.edits : state?.edit_source_ids ?? [];
  const busy = operation?.identity === identity && operation.busy;
  async function run(task: () => Promise<AgentSetupState>, notice: string | null = null) {
    if (!active || latest.current !== identity || activeTask.current?.target === identity) return;
    const generation = ++sequence.current, target = identity;
    const taskOwner = { target, generation };
    activeTask.current = taskOwner;
    setOperation({ identity: target, busy: true, error: null, notice: null });
    try {
      const value = await task();
      if (latest.current !== target || sequence.current !== generation) return;
      setResult({ identity: target, state: value });
      setSources({ identity: target, ids: value.source_ids.filter(id => value.sources.some(source => source.id === id)),
        projects: (value.reporting_project_ids ?? []).filter(id => value.projects.some(project => project.id === id)),
        edits: (value.edit_source_ids ?? []).filter(id => value.sources.some(source => source.id === id && source.editable !== false)) });
      setOperation({ identity: target, busy: false, error: null, notice });
    } catch (cause) {
      if (latest.current === target && sequence.current === generation)
        setOperation({ identity: target, busy: false, error: agentSetupError(cause), notice: null });
    } finally {
      if (activeTask.current === taskOwner) activeTask.current = null;
    }
  }
  useEffect(() => {
    latest.current = identity;
    if (active) void run(() => backend.state(selection));
    return () => { sequence.current += 1; if (latest.current === identity) latest.current = null; if (activeTask.current?.target === identity) activeTask.current = null; };
  }, [identity]);
  const permissionsChanged = !!state && (ids.length !== state.source_ids.length || ids.some(id => !state.source_ids.includes(id))
    || projectIds.length !== (state.reporting_project_ids ?? []).length || projectIds.some(id => !(state.reporting_project_ids ?? []).includes(id))
    || editIds.length !== (state.edit_source_ids ?? []).length || editIds.some(id => !(state.edit_source_ids ?? []).includes(id)));
  const pendingSelection = useRef(permissionsChanged); pendingSelection.current = permissionsChanged;
  const busyRef = useRef(busy); busyRef.current = busy;
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => {
      if (busyRef.current || activeTask.current?.target === identity || pendingSelection.current) return;
      const generation = sequence.current;
      void backend.state(selection).then(value => {
        if (latest.current === identity && sequence.current === generation && activeTask.current?.target !== identity && !pendingSelection.current)
          setResult({ identity, state: value });
      }).catch(() => {});
    }, 10_000);
    return () => window.clearInterval(timer);
  }, [identity]);
  return {
    agent, scope, project, setAgent, setScope, setProject, selection, state, ids, projectIds, editIds, busy,
    error: operation?.identity === identity ? operation.error : null,
    notice: operation?.identity === identity ? operation.notice : null,
    selectSource(id: string, checked: boolean) {
      setSources({ identity, projects: projectIds, edits: editIds, ids: checked ? [...new Set([...ids, id])] : ids.filter(value => value !== id) });
    },
    selectProject(id: string, checked: boolean) {
      if (checked && !state?.projects.some(project => project.id === id && project.enabled !== false && project.monitoring !== false)) return;
      setSources({ identity, ids, edits: editIds, projects: checked ? [...new Set([...projectIds, id])] : projectIds.filter(value => value !== id) });
    },
    selectEdit(id: string, checked: boolean) {
      if (checked && !state?.sources.some(source => source.id === id && source.editable !== false)) return;
      setSources({ identity, ids, projects: projectIds, edits: checked ? [...new Set([...editIds, id])] : editIds.filter(value => value !== id) });
    },
    canChooseFolder: !!backend.chooseProjectFolder,
    async chooseFolder() {
      if (!active || busy || !backend.chooseProjectFolder || latest.current !== identity) return;
      const generation = ++sequence.current, target = identity;
      setOperation({ identity: target, busy: true, error: null, notice: null });
      try {
        const picked = await backend.chooseProjectFolder();
        if (latest.current !== target || sequence.current !== generation) return;
        if (picked) {
          // Native saved membership is the target; no installation or read grants follow.
          setProject(picked.path);
          setScope('project');
          if (scope === 'project' && project === picked.path) {
            await run(() => backend.state(selection));
            return;
          }
        }
        setOperation({ identity: target, busy: false, error: null, notice: null });
      } catch (cause) {
        if (latest.current === target && sequence.current === generation)
          setOperation({ identity: target, busy: false, error: agentSetupError(cause), notice: null });
      }
    },
    refresh: () => run(() => backend.state(selection)),
    install: () => run(() => backend.install(selection)),
    connect: () => run(async () => {
      const installed = state?.installed ? state : await backend.install(selection);
      if (latest.current !== identity) return installed;
      if (!backend.setPermissions) throw new Error('Agent connection unavailable');
      return backend.setPermissions(selection, { enabled: true, project_ids: projectIds, read_source_ids: ids, edit_source_ids: editIds });
    }, 'Restart your agent to connect to MDReader.'),
    enable: () => run(() => backend.setPermissions ? backend.setPermissions(selection, { enabled: true, project_ids: projectIds, read_source_ids: ids, edit_source_ids: editIds }) : backend.setMcp(selection, ids, true)),
    disable: () => run(() => backend.setPermissions ? backend.setPermissions(selection, { enabled: false, project_ids: [], read_source_ids: [], edit_source_ids: [] }) : backend.setMcp(selection, [], false), 'Agent service disabled. Project indexing continues.'),
    uninstall: () => run(() => backend.uninstall(selection), 'MDReader’s owned integration entries were removed.'),
  };
}
