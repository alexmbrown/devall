import { describe, expect, it } from 'vitest';
import { envChipInfo, isHexColor, normalizeHex } from './envColor';

describe('envChipInfo', () => {
  it('uses the preset name unless a label is set', () => {
    expect(envChipInfo({ color: 'staging' })).toEqual({ preset: 'staging', text: 'staging' });
    expect(envChipInfo({ color: 'prod', label: 'EU-PROD' })).toEqual({ preset: 'prod', text: 'EU-PROD' });
  });

  it('paints custom hex colours, as a dot without a label', () => {
    expect(envChipInfo({ color: '#7c3aed', label: 'QA' })).toEqual({ color: '#7c3aed', text: 'QA' });
    expect(envChipInfo({ color: '#7c3aed' })).toEqual({ color: '#7c3aed', text: undefined });
  });

  it('falls back to a plain label chip, or nothing', () => {
    expect(envChipInfo({ label: 'Sandbox' })).toEqual({ text: 'Sandbox' });
    expect(envChipInfo({ label: '  ' })).toBeNull();
    expect(envChipInfo({})).toBeNull();
    // An unknown colour word outside a browser (no CSS.supports) is ignored rather than rendered as text.
    expect(envChipInfo({ color: 'not a colour' })).toBeNull();
  });
});

describe('hex helpers', () => {
  it('validates and normalizes', () => {
    expect(isHexColor('#abc')).toBe(true);
    expect(isHexColor('#abcd')).toBe(false);
    expect(normalizeHex('#ABC')).toBe('#aabbcc');
    expect(normalizeHex('7C3AED')).toBe('#7c3aed');
    expect(normalizeHex('blue')).toBeNull();
  });
});
