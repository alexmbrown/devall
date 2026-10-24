import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

// Tests run inside workerd (Miniflare) with the real Durable Objects, KV and WebSockets.
// The bindings below replace wrangler.toml's [vars] for tests only.
export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.toml" },
      miniflare: {
        bindings: {
          PUBLIC_DOMAIN: "devall.test",
          AUTH_MODE: "both",
          FIREBASE_PROJECT_ID: "devall-test",
          RELAY_TOKENS: "alice:alice-token-0123456789,bob:bob-token-0123456789x,sam:sam-token-0123456789xx,root:root-token-0123456789xx",
          STATIC_PLANS: JSON.stringify({ alice: "free", bob: "free", sam: "supporter", root: "selfhost" }),
          ADMIN_TOKEN: "admin-token-0123456789",
          ABUSE_EMAIL: "abuse@devall.test",
          UPGRADE_URL: "https://auth.devall.test/upgrade.html",
          INTERSTITIAL: "auto",
          // Small free-tier limits so timeouts and caps are quick to hit in tests.
          LIMITS_JSON: JSON.stringify({
            free: { requestTimeoutMs: 1500, inboxEventsPerDay: 6, inboxMaxBodyBytes: 2048, mockMaxStubs: 5, mockRequestsPerDay: 40, shares: 3, shareMaxBytes: 4096 },
            supporter: { tcpStreamsPerTunnel: 3, monitors: 2, loadRunsPerDay: 3 },
          }),
          CUSTOM_HOSTS: JSON.stringify({ "api.customer.test": "static-box" }),
          MONITOR_SECRET_KEY: "test-monitor-key-0123456789abcdef0123456789",
          MOCK_URL_STYLE: "host",
          LOAD_COLO_PROBE: "https://colo.example.test/cdn-cgi/trace",
        },
      },
    }),
  ],
  test: {
    testTimeout: 20_000,
  },
});
