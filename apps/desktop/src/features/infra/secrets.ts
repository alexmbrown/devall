/** A Secret Manager id as an environment secret name: `stripe-key` → `STRIPE_KEY`. */
export function envSecretName(secretId: string): string {
  const s = secretId
    .trim()
    .replace(/[^A-Za-z0-9_]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_+|_+$/g, '')
    .toUpperCase();
  if (!s) return 'SECRET';
  return /^[A-Z_]/.test(s) ? s : `_${s}`;
}

/** `gcp-sm://project/secret[/version]`, as written to `secretSources` (mirrors devall-infra). */
export function gcpReference(project: string, secret: string, version?: string): string {
  return version && version !== 'latest' ? `gcp-sm://${project}/${secret}/${version}` : `gcp-sm://${project}/${secret}`;
}
