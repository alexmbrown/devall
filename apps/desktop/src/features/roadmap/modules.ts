import type { Section } from '@/store/nav';

/** Roadmap modules shown in the sidebar (see docs/FEATURES.md and docs/IMPLEMENTATION_PLAN.md). */
export interface RoadmapModule {
  section: Section;
  name: string;
  milestone: string;
  group: 'free' | 'hosted';
  summary: string;
  features: string[];
  replaces: string;
}

export const ROADMAP: RoadmapModule[] = [
  {
    section: 'mocks',
    name: 'Mock server',
    milestone: 'M2',
    group: 'free',
    summary: 'A WireMock-compatible mock engine that serves the mappings in your project — no JRE, no cloud.',
    features: [
      'Stub editor, request journal and near-miss explanations (from wiremock-ui)',
      'Create a stub from any request or response, then randomize its data',
      'Templating with Faker, stateful scenarios, latency and chaos presets',
      'Proxy-and-record, OpenAPI → mocks, attach to any external WireMock',
    ],
    replaces: 'WireMock Cloud (stateful/chaos are Enterprise-only), Mockoon Cloud, Beeceptor',
  },
  {
    section: 'inspector',
    name: 'Traffic inspector',
    milestone: 'M3',
    group: 'free',
    summary: 'A local HTTPS-intercepting proxy: see what your app sends, pause it, rewrite it, turn it into mocks.',
    features: ['Local CA with one-click trust on macOS and Windows', 'Breakpoints, Map Local / Map Remote', 'Capture → stub, HAR, OpenAPI or load scenario'],
    replaces: 'Proxyman ($89), HTTP Toolkit Pro, Charles ($50), Fiddler Everywhere',
  },
  {
    section: 'webhooks',
    name: 'Webhooks',
    milestone: 'M3 local · M4 hosted',
    group: 'free',
    summary: 'Receive, verify, replay and re-sign webhooks. Local inbox is free; a public URL that buffers while your laptop sleeps is a hosted feature.',
    features: [
      'Signature verification for Stripe, GitHub, Shopify, Slack, Twilio and Standard Webhooks',
      'Re-sign on replay so 5-minute signature windows never break testing',
      'Unlimited local history; save any event as a fixture or test',
    ],
    replaces: 'webhook.site, Hookdeck developer tier, Beeceptor',
  },
  {
    section: 'tunnels',
    name: 'Tunnels',
    milestone: 'M3 BYO · M4 hosted',
    group: 'hosted',
    summary: 'Expose localhost over HTTPS. Bring your own ngrok/cloudflared/Tailscale for free, or use the hosted relay (free tier + low-cost Supporter tier), or self-host the open-source relay.',
    features: ['Stable per-project subdomains (Supporter)', 'WebSocket and SSE', 'Request inspection and replay'],
    replaces: 'ngrok ($10–20/month)',
  },
  {
    section: 'load',
    name: 'Load testing',
    milestone: 'M5',
    group: 'free',
    summary: 'Load test any saved request or flow with correct latency maths, and keep run summaries in git.',
    features: ['Open-model (arrival rate) by default', 'HDR histograms with coordinated-omission correction', 'Thresholds → CI exit codes, run comparison and baselines'],
    replaces: 'k6 Cloud ($0.15/VUh), Gatling Enterprise, Artillery Cloud',
  },
  {
    section: 'data',
    name: 'Databases & data',
    milestone: 'M5–M6',
    group: 'free',
    summary: 'Lightweight Postgres/MySQL/SQLite, Redis and MongoDB clients plus a seeded, relational fake-data generator.',
    features: ['Saved queries as files, read-only by default', 'SSH tunnels', 'One schema → consistent data in mocks, DB seeds and load tests'],
    replaces: 'TablePlus ($99), Studio 3T ($499/yr), Mockaroo ($60–7,500/yr)',
  },
  {
    section: 'infra',
    name: 'Docker & Kubernetes',
    milestone: 'M6',
    group: 'free',
    summary: 'Containers, pods, logs and a port-forward manager that fills your HTTP environment automatically.',
    features: ['Works with Docker, Podman and OrbStack sockets', 'kubeconfig auth plugins (GKE, EKS, AKS)', 'GCP emulator views: Pub/Sub, Storage, Logging'],
    replaces: 'Lens ($25–60/seat), Docker Desktop Pro features',
  },
  {
    section: 'ai',
    name: 'Local AI',
    milestone: 'M1 basics · M7 depth',
    group: 'free',
    summary: 'Bring your own model: Ollama, LM Studio, any OpenAI-compatible endpoint, or a managed llama.cpp sidecar (Metal on macOS, CPU/Vulkan on Windows).',
    features: ['Natural language → regex / JSONPath / SQL, checked against samples', 'Explain-this-failure using your own requests, mocks and logs', 'MCP server so coding agents can use your project (read-only by default)'],
    replaces: 'Postman AI credits, cloud-only AI in DB GUIs',
  },
];
