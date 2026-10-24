import { describe, expect, it } from 'vitest';
import { contributionSummary, statusTone } from './model';
import { isPluginOp, utilityToOp, type PluginManifest } from '@/lib/api/plugins';

const manifest = (contributes: PluginManifest['contributes']): PluginManifest => ({ id: 'x', name: 'X', version: '1', apiVersion: 1, capabilities: {}, contributes });

describe('plugins model', () => {
  it('summarises contributions', () => {
    expect(contributionSummary(manifest({ utilities: [{ id: 'a', name: 'A' }, { id: 'b', name: 'B' }], auth: [{ id: 'h', name: 'HMAC' }] }))).toBe('Adds 2 utilities · 1 auth provider');
    expect(contributionSummary(manifest({ hooks: [{ id: 'h', name: 'H', events: ['preRequest'] }] }))).toBe('Adds 1 request hook');
    expect(contributionSummary(manifest({}))).toBe('Adds nothing');
  });

  it('maps statuses to badge tones', () => {
    expect(statusTone('ready')).toBe('success');
    expect(statusTone('changed')).toBe('warning');
    expect(statusTone('broken')).toBe('danger');
    expect(statusTone('disabled')).toBe('outline');
  });

  it('turns plugin utilities into ops', () => {
    const op = utilityToOp({
      id: 'hmac-signer:sign',
      plugin: 'hmac-signer',
      pluginName: 'HMAC signer',
      name: 'HMAC sign',
      description: '',
      category: 'Plugins',
      output: 'text',
      noInput: false,
      args: [{ name: 'key', label: '', kind: 'secret', default: '' }],
      example: 'hello',
    });
    expect(op.description).toBe('From the HMAC signer plugin');
    expect(op.args[0].label).toBe('key');
    expect(op.examples).toEqual([{ input: 'hello', args: {} }]);
    expect(isPluginOp(op.id)).toBe(true);
    expect(isPluginOp('base64.encode')).toBe(false);
  });
});
