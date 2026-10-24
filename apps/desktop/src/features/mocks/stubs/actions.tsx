import { useQueryClient } from '@/features/mocks/lib/query';
import type { StubMapping } from '@/features/mocks/lib/types';
import { useWm, useWrite, useStubCache, qk, clientFor } from '@/features/mocks/lib/queries';
import { cleanForSave, displayName, setDisabled, stripIdentity, stubId, toggleProxy, withFolder, withWmui, tagsOf, isDisabled } from '@/features/mocks/lib/stub-utils';
import { toast } from '@/components/ui/toast';
import { confirmDialog } from '@/components/ui/dialogs';
import { useStubTabs } from '@/features/mocks/store/tabs';
import { serverKey, useMockTarget, BUILTIN_ID, type Target } from '@/features/mocks/store/target';

/** Remembers recently deleted stubs for one-click undo (in memory only). */
const trash: StubMapping[][] = [];

export function useStubActions() {
  const write = useWrite();
  const cache = useStubCache();
  const { sid, target } = useWm();
  const qc = useQueryClient();
  const inv = (s: string) => [qk.mappings(s), qk.scenarios(s), qk.unmatchedStubs(s)];
  // On the built-in engine stubs are project files: new ones are persistent unless the user says otherwise.
  const forTarget = (s: StubMapping, t: Target = target) => (t.id === BUILTIN_ID && s.persistent === undefined ? { ...s, persistent: true } : s);

  async function create(stub: StubMapping, opts: { silent?: boolean } = {}) {
    const r = await write((c) => c.createMapping(cleanForSave(forTarget(stub))), {
      label: 'Create stub',
      success: opts.silent ? undefined : 'Stub created',
      invalidate: inv,
    });
    if (r) cache.upsert(r);
    return r;
  }

  async function createMany(stubs: StubMapping[]) {
    const created: StubMapping[] = [];
    const r = await write(
      async (c) => {
        for (const s of stubs) created.push(await c.createMapping(cleanForSave(forTarget(s))));
        return created;
      },
      { label: `Create ${stubs.length} stubs`, success: `${stubs.length} stub${stubs.length === 1 ? '' : 's'} created`, invalidate: inv },
    );
    return r;
  }

  async function update(stub: StubMapping, opts: { silent?: boolean; label?: string } = {}) {
    const id = stubId(stub);
    const body = cleanForSave(stub);
    const r = await write((c) => c.updateMapping(id, body), { label: opts.label ?? 'Save stub', success: opts.silent ? undefined : 'Stub saved', invalidate: inv });
    if (r) cache.upsert(r);
    return r;
  }

  async function updateMany(stubs: StubMapping[], label: string) {
    return write(
      async (c) => {
        const out: StubMapping[] = [];
        for (const s of stubs) out.push(await c.updateMapping(stubId(s), cleanForSave(s)));
        return out;
      },
      { label, success: `${label}: ${stubs.length} stub${stubs.length === 1 ? '' : 's'}`, invalidate: inv },
    );
  }

  async function remove(stubs: StubMapping[]) {
    if (!stubs.length) return;
    const ids = stubs.map(stubId);
    const r = await write(
      async (c) => {
        for (const id of ids) await c.deleteMapping(id);
        return true;
      },
      {
        label: 'Delete stubs',
        invalidate: inv,
        confirm: {
          title: stubs.length === 1 ? `Delete "${displayName(stubs[0])}"?` : `Delete ${stubs.length} stubs?`,
          message: stubs.some((s) => s.persistent) ? 'Persistent stubs are also removed from the server mappings folder.' : 'You can undo right after deleting.',
          danger: true,
          confirmText: 'Delete',
        },
      },
    );
    if (!r) return;
    cache.remove(ids);
    trash.push(stubs.map((s) => structuredClone(s)));
    const ws = useStubTabs.getState();
    for (const id of ids) {
      const t = ws.findByStub(sid, id);
      if (t) ws.close(sid, t.key);
    }
    toast.success(stubs.length === 1 ? 'Stub deleted' : `${stubs.length} stubs deleted`, undefined, {
      label: 'Undo',
      run: () => undoDelete(),
    });
  }

  async function undoDelete() {
    const batch = trash.pop();
    if (!batch) return;
    await write(
      async (c) => {
        // Re-create with the same ids so links / tabs keep working.
        await c.importMappings(batch.map(cleanForSave), 'OVERWRITE');
      },
      { label: 'Restore stubs', success: `Restored ${batch.length} stub${batch.length === 1 ? '' : 's'}`, invalidate: inv },
    );
  }

  async function duplicate(stub: StubMapping) {
    const copy = stripIdentity(stub);
    copy.name = `${displayName(stub)} (copy)`;
    return create(copy);
  }

  const bulk = {
    disable: (stubs: StubMapping[], disabled: boolean) => updateMany(stubs.filter((s) => isDisabled(s) !== disabled).map((s) => setDisabled(s, disabled)), disabled ? 'Disabled' : 'Enabled'),
    toggleProxy: (stubs: StubMapping[]) => updateMany(stubs.map(toggleProxy), 'Toggled proxy'),
    move: (stubs: StubMapping[], folder: string) => updateMany(stubs.map((s) => withFolder(s, folder)), `Moved to ${folder || 'root'}`),
    tag: (stubs: StubMapping[], tag: string) => updateMany(stubs.map((s) => withWmui(s, { tags: [...new Set([...tagsOf(s), tag])] })), `Tagged "${tag}"`),
    untag: (stubs: StubMapping[], tag: string) => updateMany(stubs.map((s) => withWmui(s, { tags: tagsOf(s).filter((t) => t !== tag) })), `Removed tag "${tag}"`),
    persistent: (stubs: StubMapping[], persistent: boolean) => updateMany(stubs.map((s) => ({ ...s, persistent: persistent || undefined })), persistent ? 'Marked persistent' : 'Marked non-persistent'),
  };

  /** Copy stubs to another server (keeps ids so repeated copies overwrite). */
  async function copyTo(to: Target, stubs: StubMapping[]) {
    if (to.readOnly) return toast.error('Target server is read-only');
    const c = clientFor(to);
    if (!c) return toast.error(`${to.name} is not running`, to.id === BUILTIN_ID ? 'Start the built-in mock server first.' : undefined);
    if (to.protected) {
      const ok = await confirmDialog({ title: `Copy ${stubs.length} stub(s) to ${to.name}?`, message: `"${to.name}" is a protected server. Stubs with the same id are overwritten.`, confirmText: 'Copy', typeToConfirm: to.name });
      if (!ok) return;
    }
    try {
      await c.importMappings(stubs.map((s) => cleanForSave(forTarget(s, to))), 'OVERWRITE');
      if (to.id === BUILTIN_ID) useMockTarget.getState().markLocalWrite();
      toast.success(`Copied ${stubs.length} stub${stubs.length === 1 ? '' : 's'} to ${to.name}`);
      qc.invalidateQueries({ queryKey: qk.all(serverKey(to)) });
      if (to.id === BUILTIN_ID) void useMockTarget.getState().refreshStatus();
    } catch (e) {
      toast.error(`Copy to ${to.name} failed`, e);
    }
  }

  return { create, createMany, update, updateMany, remove, duplicate, undoDelete, bulk, copyTo };
}
