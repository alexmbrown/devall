/**
 * Registers WireMock's stub-mapping JSON schema with Monaco's JSON language service, next to the
 * app's own settings (comments allowed, no remote schema requests). Stub editors opt in through
 * their model path (`stubModelPath`); other JSON editors are unaffected.
 */
import { monaco } from '@/components/editor/monaco';
import stubSchema3 from './schema/wiremock-3.schema.json';
import { GRAPHQL_MATCHER_SCHEMA } from '@/features/mocks/protocols/schemas';

export const STUB_MODEL_PREFIX = 'inmemory://devall-mocks/stub/';
export const STUBS_ARRAY_MODEL_PREFIX = 'inmemory://devall-mocks/stubs/';

let registered = false;

function register() {
  if (registered) return;
  registered = true;
  const defs = (stubSchema3 as { definitions: Record<string, unknown> }).definitions;
  // /dev/all's `graphql` request matcher (WireMock leaves unknown request keys alone).
  const rp = defs['request-pattern'] as { properties?: Record<string, unknown> } | undefined;
  if (rp?.properties && !rp.properties.graphql) rp.properties.graphql = GRAPHQL_MATCHER_SCHEMA;
  const ours = [
    { uri: 'https://wiremock.org/schemas/3/stub-mapping.json', fileMatch: [`${STUB_MODEL_PREFIX}v3/*`], schema: stubSchema3 },
    {
      uri: 'https://wiremock.org/schemas/3/stub-mappings.json',
      fileMatch: [`${STUBS_ARRAY_MODEL_PREFIX}v3/*`],
      schema: {
        definitions: defs,
        oneOf: [
          { type: 'object', properties: { mappings: { type: 'array', items: { $ref: '#/definitions/stub-mapping' } } } },
          { type: 'array', items: { $ref: '#/definitions/stub-mapping' } },
        ],
      },
    },
  ];
  const current = monaco.json.jsonDefaults.diagnosticsOptions;
  const others = (current.schemas ?? []).filter((s) => !ours.some((o) => o.uri === s.uri));
  monaco.json.jsonDefaults.setDiagnosticsOptions({ ...current, validate: true, enableSchemaRequest: false, schemas: [...others, ...ours] });
}

/**
 * Model path for a stub editor. Only the 3.x schema is bundled (the built-in engine reports 3.13);
 * stubs of a 4.x server get plain JSON validation so 4.x-only fields are not flagged.
 */
export function stubModelPath(schema: 3 | 4, key: string): string {
  register();
  return schema === 3 ? `${STUB_MODEL_PREFIX}v3/${key}.json` : `${STUB_MODEL_PREFIX}v${schema}-unvalidated/${key}.json`;
}
