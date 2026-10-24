import { describe, expect, it } from 'vitest';
import type { TunnelStatus } from '@/lib/api/webhooks';
import { installGuide, platformOf, portChoices, sortTunnels, validPort } from './byo';

describe('platformOf', () => {
  it('detects the OS', () => {
    expect(platformOf('Mozilla/5.0 (Windows NT 10.0; Win64; x64)', 'Win32')).toBe('windows');
    expect(platformOf('Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5)', 'MacIntel')).toBe('macos');
    expect(platformOf('Mozilla/5.0 (X11; Linux x86_64)', 'Linux x86_64')).toBe('linux');
  });
});

describe('installGuide', () => {
  it('gives package-manager commands per platform', () => {
    expect(installGuide('ngrok', 'windows').options.map((o) => o.command)).toEqual(['winget install ngrok.ngrok', 'scoop install ngrok', 'choco install ngrok']);
    expect(installGuide('ngrok', 'macos').options[0].command).toBe('brew install ngrok');
    expect(installGuide('ngrok', 'macos').then.join(' ')).toMatch(/keychain/);
    expect(installGuide('cloudflared', 'windows').options[0].command).toBe('winget install --id Cloudflare.cloudflared');
    expect(installGuide('tailscale', 'macos').options[0].command).toBe('brew install --cask tailscale');
  });
});

describe('portChoices', () => {
  it('lists running servers once, flagging protocols HTTP tunnels cannot carry', () => {
    const c = portChoices(
      [
        { running: true, port: 8080, protocols: { grpcPort: 50051, mqttPort: 1883 } },
        { running: false, port: 9090, protocols: null },
      ],
      4590,
    );
    expect(c.map((x) => [x.port, x.label])).toEqual([
      [8080, 'Mock server'],
      [50051, 'gRPC mock'],
      [1883, 'MQTT mock'],
      [4590, 'Webhook receiver'],
    ]);
    expect(c.find((x) => x.port === 1883)?.unsupported).toBeTruthy();
    expect(c.find((x) => x.port === 50051)?.note).toMatch(/http2/);
    expect(portChoices([{ running: true, port: 4590, protocols: null }], 4590)).toHaveLength(1);
  });
});

describe('helpers', () => {
  it('sorts live tunnels first', () => {
    const t = (id: string, state: TunnelStatus['state'], startedAt: number) => ({ id, state, startedAt }) as TunnelStatus;
    expect(sortTunnels([t('a', 'error', 3), t('b', 'online', 1), t('c', 'starting', 2), t('d', 'online', 5)]).map((x) => x.id)).toEqual(['d', 'b', 'c', 'a']);
  });

  it('validates ports', () => {
    expect(validPort('3000')).toBe(3000);
    expect(validPort(' 8080 ')).toBe(8080);
    expect(validPort('0')).toBeNull();
    expect(validPort('70000')).toBeNull();
    expect(validPort('abc')).toBeNull();
  });
});
