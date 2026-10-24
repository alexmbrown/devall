import type { TunnelKind, TunnelStatus } from '@/lib/api/webhooks';

// Pure helpers for bring-your-own tunnels (unit-tested in byo.test.ts).

export type Platform = 'windows' | 'macos' | 'linux';

export function platformOf(ua: string, platform = ''): Platform {
  const s = `${platform} ${ua}`;
  if (/Win/i.test(s)) return 'windows';
  if (/Mac|iPhone|iPad/i.test(s)) return 'macos';
  return 'linux';
}

export interface InstallOption {
  /** Package manager or method. */
  via: string;
  command?: string;
  url?: string;
}

export interface InstallGuide {
  options: InstallOption[];
  /** What to do after installing (account, token, admin console). */
  then: string[];
}

const NGROK_THEN = [
  'Sign up for a free account at dashboard.ngrok.com and copy your authtoken (Getting Started → Your Authtoken).',
  'Paste it in the ngrok settings here — it is stored in your OS keychain and passed as NGROK_AUTHTOKEN, never written to a file.',
  'Optional: claim your free static domain (dashboard → Domains) and enter it as the reserved domain for a stable webhook URL.',
];
const TAILSCALE_THEN = [
  'Open Tailscale and log in.',
  'In the admin console enable HTTPS certificates (DNS page) and allow Funnel for this device (Access controls → nodeAttrs "funnel").',
];

const GUIDES: Record<TunnelKind, Record<Platform, InstallGuide>> = {
  ngrok: {
    windows: {
      options: [
        { via: 'winget', command: 'winget install ngrok.ngrok' },
        { via: 'Scoop', command: 'scoop install ngrok' },
        { via: 'Chocolatey', command: 'choco install ngrok' },
      ],
      then: NGROK_THEN,
    },
    macos: { options: [{ via: 'Homebrew', command: 'brew install ngrok' }, { via: 'Download', url: 'https://ngrok.com/download' }], then: NGROK_THEN },
    linux: { options: [{ via: 'Snap', command: 'sudo snap install ngrok' }, { via: 'Download', url: 'https://ngrok.com/download' }], then: NGROK_THEN },
  },
  cloudflared: {
    windows: {
      options: [
        { via: 'winget', command: 'winget install --id Cloudflare.cloudflared' },
        { via: 'Scoop', command: 'scoop install cloudflared' },
        { via: 'Chocolatey', command: 'choco install cloudflared' },
      ],
      then: ['Quick tunnels need no Cloudflare account — click Expose.'],
    },
    macos: { options: [{ via: 'Homebrew', command: 'brew install cloudflared' }], then: ['Quick tunnels need no Cloudflare account — click Expose.'] },
    linux: {
      options: [{ via: 'Package', url: 'https://pkg.cloudflare.com/index.html' }, { via: 'Download', url: 'https://github.com/cloudflare/cloudflared/releases' }],
      then: ['Quick tunnels need no Cloudflare account — click Expose.'],
    },
  },
  tailscale: {
    windows: {
      options: [
        { via: 'winget', command: 'winget install tailscale.tailscale' },
        { via: 'Chocolatey', command: 'choco install tailscale' },
      ],
      then: TAILSCALE_THEN,
    },
    macos: { options: [{ via: 'Homebrew', command: 'brew install --cask tailscale' }, { via: 'Download', url: 'https://tailscale.com/download' }], then: TAILSCALE_THEN },
    linux: { options: [{ via: 'Script', command: 'curl -fsSL https://tailscale.com/install.sh | sh' }], then: TAILSCALE_THEN },
  },
};

export function installGuide(kind: TunnelKind, platform: Platform): InstallGuide {
  return GUIDES[kind][platform];
}

/** A local server worth exposing. */
export interface PortChoice {
  port: number;
  label: string;
  /** Why an HTTP tunnel can't carry it (MQTT is raw TCP). */
  unsupported?: string;
  /** Extra setup the tunnel needs for this protocol. */
  note?: string;
}

/** Quick picks from what runs in the app (mock servers incl. gRPC/MQTT listeners, webhook receiver). */
export function portChoices(mocks: { running: boolean; port: number | null; protocols: { grpcPort: number | null; mqttPort: number | null } | null }[], receiverPort?: number | null): PortChoice[] {
  const out: PortChoice[] = [];
  for (const m of mocks) {
    if (!m.running) continue;
    if (m.port) out.push({ port: m.port, label: 'Mock server' });
    if (m.protocols?.grpcPort) out.push({ port: m.protocols.grpcPort, label: 'gRPC mock', note: 'gRPC needs HTTP/2 end to end: with ngrok add --app-protocol=http2 to extra arguments.' });
    if (m.protocols?.mqttPort) out.push({ port: m.protocols.mqttPort, label: 'MQTT mock', unsupported: 'MQTT is raw TCP; HTTP tunnels can’t carry it. Use a hosted TCP tunnel instead.' });
  }
  if (receiverPort) out.push({ port: receiverPort, label: 'Webhook receiver' });
  const seen = new Set<number>();
  return out.filter((c) => (seen.has(c.port) ? false : (seen.add(c.port), true)));
}

/** Running tunnels first (online, then starting), then failed/stopped; newest first within a group. */
export function sortTunnels(list: TunnelStatus[]): TunnelStatus[] {
  const rank = (s: TunnelStatus) => ({ online: 0, starting: 1, error: 2, stopped: 3 })[s.state];
  return [...list].sort((a, b) => rank(a) - rank(b) || b.startedAt - a.startedAt);
}

export function validPort(v: string): number | null {
  const n = Number(v.trim());
  return Number.isInteger(n) && n > 0 && n < 65536 ? n : null;
}
