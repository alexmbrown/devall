import { useEffect, useState } from 'react';
import { create } from 'zustand';
import { api } from '@/lib/ipc';
import type { VarInfo } from '@/lib/types';
import { useWorkspace } from './workspace';
import { usePrefs } from './prefs';

/** Bumped whenever variables may have changed (environment saved, project refreshed). */
export const useVarsVersion = create<{ v: number; bump(): void }>((set) => ({ v: 0, bump: () => set((s) => ({ v: s.v + 1 })) }));

const cache = new Map<string, Promise<VarInfo[]>>();

/** Variables visible to the request at `path` under the active environment. */
export function useVariables(path: string): VarInfo[] {
  const root = useWorkspace((s) => s.project?.root);
  const env = usePrefs((s) => (root ? s.activeEnv[root] : undefined));
  const version = useVarsVersion((s) => s.v);
  const [vars, setVars] = useState<VarInfo[]>([]);
  useEffect(() => {
    if (!root) return;
    const key = `${version}|${root}|${path}|${env ?? ''}`;
    let p = cache.get(key);
    if (!p) {
      if (cache.size > 200) cache.clear();
      p = api.variableCatalog(root, path, env).catch(() => []);
      cache.set(key, p);
    }
    let live = true;
    p.then((v) => live && setVars(v));
    return () => {
      live = false;
    };
  }, [root, path, env, version]);
  return vars;
}
