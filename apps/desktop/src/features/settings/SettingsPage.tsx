import { useEffect, useRef } from 'react';
import { Field, Segmented, Switch } from '@/components/ui/misc';
import { useNav } from '@/store/nav';
import { usePrefs, type Density, type ReqLayout, type Theme } from '@/store/prefs';
import { useWorkspace } from '@/store/workspace';
import { SidebarSettings } from './SidebarSettings';
import { TunnelSettingsSection } from '@/features/tunnels/ByoComponents';
import { HistorySettingsSection } from './HistorySettings';

export default function SettingsPage() {
  const p = usePrefs();
  const app = useWorkspace((s) => s.app);
  const param = useNav((s) => s.param);
  const sidebarRef = useRef<HTMLDivElement>(null);
  const historyRef = useRef<HTMLDivElement>(null);
  // `go('settings', 'sidebar')` (sidebar context menu → Customize sidebar…) and
  // `go('settings', 'history')` (History page) scroll to that section.
  useEffect(() => {
    const el = param === 'sidebar' ? sidebarRef.current : param === 'history' ? historyRef.current : null;
    const body = el?.closest('.page-body');
    if (el && body) body.scrollTo({ top: body.scrollTop + el.getBoundingClientRect().top - body.getBoundingClientRect().top - 16, behavior: 'smooth' });
  }, [param]);
  return (
    <div className="page">
      <div className="page-header">
        <h1>Settings</h1>
      </div>
      <div className="page-body narrow col-lg" style={{ maxWidth: 640 }}>
        <Field label="Theme">
          <Segmented<Theme>
            value={p.theme}
            onChange={(v) => p.set('theme', v)}
            options={[
              { value: 'system', label: 'System' },
              { value: 'light', label: 'Light' },
              { value: 'dark', label: 'Dark' },
            ]}
          />
        </Field>
        <Field label="Density">
          <Segmented<Density>
            value={p.density}
            onChange={(v) => p.set('density', v)}
            options={[
              { value: 'compact', label: 'Compact' },
              { value: 'comfortable', label: 'Comfortable' },
            ]}
          />
        </Field>
        <Field label="Request editor: response position" hint="Automatic puts the response under the request when the editor is narrow (small window or split view).">
          <Segmented<ReqLayout>
            value={p.reqLayout}
            onChange={(v) => p.set('reqLayout', v)}
            options={[
              { value: 'auto', label: 'Automatic' },
              { value: 'columns', label: 'Right' },
              { value: 'rows', label: 'Below' },
            ]}
          />
        </Field>
        <Switch checked={p.autoSave} onChange={(v) => p.set('autoSave', v)} label="Auto-save request edits (otherwise Ctrl+S)" />
        <Switch checked={p.wordWrap} onChange={(v) => p.set('wordWrap', v)} label="Wrap long lines in editors" />
        <Switch
          checked={p.clipboardDetect}
          onChange={(v) => p.set('clipboardDetect', v)}
          label="Detect clipboard on focus (Utilities page)"
          title="When the window regains focus on the Utilities page, check the clipboard for a JWT, timestamp, Base64… and offer to decode it. Nothing leaves this machine."
        />
        <hr />
        <div ref={sidebarRef}>
          <SidebarSettings />
        </div>
        <hr />
        <TunnelSettingsSection />
        <hr />
        <div ref={historyRef}>
          <HistorySettingsSection />
        </div>
        <hr />
        <dl className="kv">
          <dt>Version</dt>
          <dd>{app?.version}</dd>
          <dt>Platform</dt>
          <dd>{app?.platform}</dd>
          <dt>Secret storage</dt>
          <dd>{app?.keychainAvailable ? 'OS keychain' : 'Unavailable — in-memory only (use .env files)'}</dd>
          <dt>Scratch pad</dt>
          <dd className="mono xs break">{app?.scratchPath}</dd>
          <dt>App settings</dt>
          <dd className="mono xs break">{app?.configDir}</dd>
        </dl>
        <p className="subtle xs">No account, no telemetry. Everything above runs locally and is free forever.</p>
      </div>
    </div>
  );
}
