import { describe, expect, it } from 'vitest';
import { envSecretName, gcpReference } from './secrets';

describe('Secret Manager import helpers', () => {
  it('derives environment secret names', () => {
    expect(envSecretName('stripe-key')).toBe('STRIPE_KEY');
    expect(envSecretName('db.password--prod')).toBe('DB_PASSWORD_PROD');
    expect(envSecretName('3rd-party')).toBe('_3RD_PARTY');
    expect(envSecretName('---')).toBe('SECRET');
  });

  it('builds provider references', () => {
    expect(gcpReference('shop', 'stripe-key')).toBe('gcp-sm://shop/stripe-key');
    expect(gcpReference('shop', 'stripe-key', 'latest')).toBe('gcp-sm://shop/stripe-key');
    expect(gcpReference('shop', 'stripe-key', '4')).toBe('gcp-sm://shop/stripe-key/4');
  });
});
