// Pure logic of the hosted extras: JSONPath, the WireMock-subset matcher and templater, monitor
// assertions and secret encryption, custom-host parsing.

import { describe, expect, it } from "vitest";
import { parseCustomHosts } from "../src/customhosts";
import { isDefinite, queryJsonPath, readJsonPath } from "../src/jsonpath";
import { compileStub, findMatch, jsonEquals, sortStubs, type MockRequest, type Warning } from "../src/mockmatch";
import { applyOffset, Bag, formatDate, Multi, PathValue, renderTemplate, supportedHelpers, TemplateError } from "../src/mocktemplate";
import { accountKey, checkAssertion, decryptSecret, encryptSecret, isDue, runCheck, validateMonitor, type MonitorRequest } from "../src/monitor";
import { isValidHostname } from "../src/names";
import { validateSnapshot } from "../src/share";
import subset from "../src/hosted-mock-subset.json";

const doc = {
  store: {
    book: [
      { title: "A", price: 8.95, tags: ["x"] },
      { title: "B", price: 12.99, isbn: "0-1" },
      { title: "C", price: 22.99, isbn: "0-2" },
    ],
    bicycle: { color: "red", price: 19.95 },
  },
  "odd key": 1,
};

describe("jsonpath", () => {
  it("selects children, indexes, slices, wildcards and descendants", () => {
    expect(queryJsonPath(doc, "$.store.bicycle.color")).toEqual(["red"]);
    expect(queryJsonPath(doc, "$['odd key']")).toEqual([1]);
    expect(queryJsonPath(doc, "$.store.book[1].title")).toEqual(["B"]);
    expect(queryJsonPath(doc, "$.store.book[-1].title")).toEqual(["C"]);
    expect(queryJsonPath(doc, "$.store.book[0,2].title")).toEqual(["A", "C"]);
    expect(queryJsonPath(doc, "$.store.book[:2].title")).toEqual(["A", "B"]);
    expect(queryJsonPath(doc, "$.store.book[*].price")).toEqual([8.95, 12.99, 22.99]);
    expect(queryJsonPath(doc, "$..price")).toEqual([8.95, 12.99, 22.99, 19.95]);
    expect(queryJsonPath(doc, "$.store.book.length")).toEqual([3]);
    expect(queryJsonPath(doc, "store.bicycle.price")).toEqual([19.95]);
  });

  it("filters like Jayway", () => {
    expect(queryJsonPath(doc, "$.store.book[?(@.price < 10)].title")).toEqual(["A"]);
    expect(queryJsonPath(doc, "$.store.book[?(@.isbn)].title")).toEqual(["B", "C"]);
    expect(queryJsonPath(doc, "$.store.book[?(@.title == 'B' || @.price > 20)].title")).toEqual(["B", "C"]);
    expect(queryJsonPath(doc, "$.store.book[?(@.title =~ /^[AB]$/ && @.price > 9)].title")).toEqual(["B"]);
    expect(queryJsonPath({ id: 1, name: "x" }, "$[?(@.name == 'x')]")).toEqual([{ id: 1, name: "x" }]);
  });

  it("knows definite paths and reads single values", () => {
    expect(isDefinite("$.a.b[0]")).toBe(true);
    expect(isDefinite("$.a[*]")).toBe(false);
    expect(readJsonPath(doc, "$.store.bicycle.color")).toBe("red");
    expect(readJsonPath(doc, "$.nope")).toBeUndefined();
    expect(readJsonPath(doc, "$..isbn")).toEqual(["0-1", "0-2"]);
    expect(() => queryJsonPath(doc, "$.a[")).toThrow();
  });
});

function req(over: Partial<MockRequest> = {}): MockRequest {
  const body = over.bodyText ?? "";
  return {
    method: "GET",
    url: "/",
    path: "/",
    query: [],
    headers: [],
    cookies: [],
    body: new TextEncoder().encode(body),
    bodyText: body,
    host: "pets.mock.devall.test",
    ...over,
  };
}

function compile(mappings: Record<string, unknown>[]) {
  const warnings: Warning[] = [];
  const stubs = sortStubs(mappings.map((m, i) => compileStub(m, i, warnings)));
  const state = new Map<string, string>();
  return {
    warnings,
    match: (r: MockRequest) => findMatch(stubs, r, (n) => state.get(n) ?? "Started"),
    state,
  };
}

describe("mock matcher (WireMock subset)", () => {
  it("matches urls, methods, priorities and recency", () => {
    const m = compile([
      { name: "fallback", priority: 10, request: { method: "ANY", urlPattern: "/.*" }, response: { status: 404 } },
      { name: "exact", priority: 1, request: { method: "GET", url: "/pets?limit=2" }, response: { status: 200 } },
      { name: "path", request: { method: "GET", urlPath: "/pets" }, response: { status: 200 } },
      { name: "newer-path", request: { method: "GET", urlPath: "/pets" }, response: { status: 200 } },
      { name: "template", request: { method: "GET", urlPathTemplate: "/pets/{id}/toys/{toy}" }, response: { status: 200 } },
      { name: "pathPattern", request: { method: "DELETE", urlPathPattern: "/pets/[0-9]+" }, response: { status: 204 } },
    ]);
    expect(m.warnings).toEqual([]);
    expect(m.match(req({ url: "/pets?limit=2", path: "/pets", query: [["limit", "2"]] }))?.stub.name).toBe("exact");
    expect(m.match(req({ url: "/pets?limit=3", path: "/pets" }))?.stub.name).toBe("newer-path");
    const t = m.match(req({ url: "/pets/7/toys/ball%20x", path: "/pets/7/toys/ball%20x" }));
    expect(t?.stub.name).toBe("template");
    expect(t?.pathParams).toEqual({ id: "7", toy: "ball x" });
    expect(m.match(req({ method: "DELETE", url: "/pets/12", path: "/pets/12" }))?.stub.name).toBe("pathPattern");
    expect(m.match(req({ method: "DELETE", url: "/pets/x", path: "/pets/x" }))?.stub.name).toBe("fallback");
  });

  it("matches headers, query, cookies, form, basic auth and host", () => {
    const m = compile([
      {
        name: "all",
        request: {
          method: "POST",
          urlPath: "/login",
          headers: { "Content-Type": { contains: "form" }, "X-Trace": { absent: true }, Accept: { matches: "(?i)application/.*" } },
          queryParameters: { v: { equalTo: "1" } },
          cookies: { session: { matches: "s-[0-9]+" } },
          formParameters: { user: { equalTo: "ada", caseInsensitive: true } },
          basicAuthCredentials: { username: "u", password: "p" },
          host: { equalTo: "pets.mock.devall.test" },
        },
        response: { status: 200 },
      },
    ]);
    const good = req({
      method: "POST",
      url: "/login?v=1",
      path: "/login",
      query: [["v", "1"]],
      headers: [
        ["content-type", "application/x-www-form-urlencoded"],
        ["accept", "APPLICATION/json"],
        ["authorization", `Basic ${btoa("u:p")}`],
      ],
      cookies: [["session", "s-42"]],
      bodyText: "user=ADA&x=1",
    });
    expect(m.match(good)?.stub.name).toBe("all");
    expect(m.match({ ...good, headers: [...good.headers, ["x-trace", "1"]] })).toBeNull();
    expect(m.match({ ...good, cookies: [["session", "nope"]] })).toBeNull();
    expect(m.match({ ...good, host: "other.example" })).toBeNull();
    expect(m.match({ ...good, headers: good.headers.filter(([k]) => k !== "authorization") })).toBeNull();
  });

  it("matches JSON bodies (equalToJson options, placeholders, matchesJsonPath)", () => {
    const m = compile([
      { name: "json", request: { method: "POST", bodyPatterns: [{ equalToJson: { a: [1, 2], b: "${json-unit.any-string}" }, ignoreArrayOrder: true, ignoreExtraElements: true }] }, response: {} },
      { name: "path", request: { method: "PUT", bodyPatterns: [{ matchesJsonPath: "$.items[?(@.qty > 2)]" }, { matchesJsonPath: { expression: "$.user.name", matches: "A.*" } }, { contains: "items" }] }, response: {} },
    ]);
    expect(m.match(req({ method: "POST", bodyText: '{"a":[2,1],"b":"x","c":true}' }))?.stub.name).toBe("json");
    expect(m.match(req({ method: "POST", bodyText: '{"a":[2,1],"b":5}' }))).toBeNull();
    expect(m.match(req({ method: "PUT", bodyText: '{"items":[{"qty":1},{"qty":3}],"user":{"name":"Ada"}}' }))?.stub.name).toBe("path");
    expect(m.match(req({ method: "PUT", bodyText: '{"items":[{"qty":1}],"user":{"name":"Ada"}}' }))).toBeNull();
    expect(jsonEquals([1, [2, 3]], [1, [2, 3]], { ignoreArrayOrder: false, ignoreExtraElements: false })).toBe(true);
    expect(jsonEquals({ a: 1 }, { a: 1, b: 2 }, { ignoreArrayOrder: false, ignoreExtraElements: false })).toBe(false);
  });

  it("follows scenarios", () => {
    const m = compile([
      { name: "first", scenarioName: "s", requiredScenarioState: "Started", newScenarioState: "two", request: { urlPath: "/x" }, response: {} },
      { name: "second", scenarioName: "s", requiredScenarioState: "two", request: { urlPath: "/x" }, response: {} },
    ]);
    const r = req({ url: "/x", path: "/x" });
    const a = m.match(r)!;
    expect(a.stub.name).toBe("first");
    m.state.set("s", a.stub.newScenarioState!);
    expect(m.match(r)?.stub.name).toBe("second");
  });

  it("implements every matcher the shared subset file lists", () => {
    const samples: Record<string, unknown> = {
      equalTo: "x", contains: "x", doesNotContain: "y", matches: "x", doesNotMatch: "y", absent: false, anything: "anything",
      equalToJson: "{}", matchesJsonPath: "$", binaryEqualTo: "eA==", and: [{ contains: "x" }], or: [{ contains: "x" }], not: { contains: "y" },
    };
    expect(Object.keys(samples).sort()).toEqual([...subset.matchers].sort());
    for (const k of subset.matchers) {
      const m = compile([{ name: k, request: { headers: { "X-Probe": { [k]: samples[k] } } }, response: {} }]);
      expect(m.warnings, k).toEqual([]);
    }
  });

  it("fails closed on unsupported matchers and reports what's ignored", () => {
    const m = compile([
      { name: "xml", request: { method: "POST", bodyPatterns: [{ equalToXml: "<a/>" }] }, response: { status: 200 } },
      { name: "multipart", request: { multipartPatterns: [] }, response: {} },
      { name: "proxy", request: { urlPath: "/p" }, response: { proxyBaseUrl: "https://example.com", chunkedDribbleDelay: { numberOfChunks: 2 }, transformers: ["my-ext"] } },
    ]);
    expect(m.match(req({ method: "POST", bodyText: "<a/>" }))).toBeNull();
    const features = m.warnings.map((w) => `${w.stub}:${w.feature}`);
    expect(features).toEqual(
      expect.arrayContaining(["xml:request", "multipart:request.multipartPatterns", "proxy:response.proxyBaseUrl", "proxy:response.chunkedDribbleDelay", "proxy:response.transformers"]),
    );
  });
});

function model(over: { query?: [string, string][]; headers?: [string, string][]; body?: string; path?: string; vars?: Record<string, string> } = {}) {
  const query: Record<string, Multi> = {};
  for (const [k, v] of over.query ?? []) (query[k] ??= new Multi([])).values.push(v);
  const path = over.path ?? "/things/7";
  return {
    request: {
      url: `${path}?q=1`,
      path: new PathValue(path, path.split("/").filter(Boolean), over.vars ?? {}),
      pathSegments: path.split("/").filter(Boolean),
      query,
      headers: new Bag(over.headers ?? [["X-Name", "Ada"]], true),
      cookies: {},
      method: "POST",
      body: over.body ?? '{"name":"bob","items":[{"id":1},{"id":2}],"nested":{"x":1}}',
      baseUrl: "https://pets.mock.devall.test",
    },
    parameters: { greeting: "hi" },
  };
}

const NOW = Date.UTC(2026, 9, 22, 7, 8, 9, 123);

describe("mock templating (Handlebars subset)", () => {
  const r = (tpl: string, m = model()) => renderTemplate(tpl, m, () => NOW);

  it("renders the request model", () => {
    expect(r("{{request.method}} {{request.path}} {{request.pathSegments.[1]}} {{request.path.[0]}}")).toBe("POST /things/7 7 things");
    expect(r("{{request.path.id}}", model({ vars: { id: "42" } }))).toBe("42");
    expect(r("{{request.headers.x-name}} {{request.headers.[X-NAME]}}")).toBe("Ada Ada");
    expect(r("{{request.query.tag}} {{request.query.tag.[1]}} {{size request.query.tag}}", model({ query: [["tag", "a"], ["tag", "b"]] }))).toBe("[a, b] b 2");
    expect(r("{{parameters.greeting}} {{request.baseUrl}}")).toBe("hi https://pets.mock.devall.test");
    expect(r("{{{request.body}}}")).toBe('{"name":"bob","items":[{"id":1},{"id":2}],"nested":{"x":1}}');
  });

  it("supports jsonPath, blocks and conditions", () => {
    expect(r("{{jsonPath request.body '$.name'}}")).toBe("bob");
    expect(r("{{jsonPath request.body '$.nested'}}")).toBe('{"x":1}');
    expect(r("{{jsonPath request.body '$.missing' default='none'}}")).toBe("none");
    expect(r("{{#each (jsonPath request.body '$.items')}}{{@index}}:{{this.id}}{{#unless @last}},{{/unless}}{{/each}}")).toBe("0:1,1:2");
    expect(r("{{#if (eq request.method 'POST')}}yes{{else}}no{{/if}}")).toBe("yes");
    expect(r("{{#eq request.method 'GET'}}get{{else}}other{{/eq}}")).toBe("other");
    expect(r("{{#if (and (gt 3 2) (contains request.body 'bob'))}}ok{{/if}}")).toBe("ok");
    expect(r("{{#if false}}a{{else if true}}b{{else}}c{{/if}}")).toBe("b");
    expect(r("{{#each (range 1 3)}}{{this}}{{/each}}")).toBe("123");
    expect(r("{{join ',' (array 'a' 'b')}} {{math 2 '*' 21}} {{upper 'x'}} {{replace 'a-b' '-' '+'}}")).toBe("a,b 42 X a+b");
    expect(r("{{! comment }}a  {{~ 'b' ~}}  c")).toBe("abc");
    expect(r("\\{{literal}}")).toBe("{{literal}}");
  });

  it("formats dates and random values", () => {
    expect(r("{{now}}")).toBe("2026-10-22T07:08:09Z");
    expect(r("{{now format='yyyy-MM-dd HH:mm:ss.SSS'}}")).toBe("2026-10-22 07:08:09.123");
    expect(r("{{now offset='3 days' format='yyyy-MM-dd'}}")).toBe("2026-10-25");
    expect(r("{{now format='epoch'}}")).toBe(String(NOW));
    expect(formatDate(applyOffset(NOW, "-1 months"), "dd MMM yyyy")).toBe("22 Sep 2026");
    expect(r("{{randomValue length=12 type='HEXADECIMAL'}}")).toMatch(/^[0-9a-f]{12}$/);
    expect(r("{{randomValue type='UUID'}}")).toMatch(/^[0-9a-f-]{36}$/);
    const n = Number(r("{{randomInt lower=5 upper=6}}"));
    expect(n === 5 || n === 6).toBe(true);
    expect(r("{{random 'Name.firstName'}}")).toMatch(/^\p{L}+$/u);
    expect(r("{{random 'internet.email_address'}}")).toMatch(/@example\.com$/);
  });

  it("refuses unsupported helpers and runaway templates", () => {
    expect(() => r("{{xPath request.body '/a'}}")).toThrow(TemplateError);
    expect(() => r("{{#each (range 1 20000)}}x{{/each}}")).toThrow(/range/);
    expect(() => r("{{#if x}}open")).toThrow(/missing/);
    expect(supportedHelpers()).toEqual(expect.arrayContaining(["jsonPath", "now", "randomValue", "random", "each", "if"]));
    // The desktop's publish check reads the same list (crates/devall-relay/src/hosted_mock.rs).
    expect(supportedHelpers()).toEqual([...subset.helpers].sort());
  });
});

describe("monitors", () => {
  const ctx = { status: 200, headers: new Headers({ "content-type": "application/json", "x-a": "1" }), body: '{"items":[1,2],"name":"x","ok":true}', durationMs: 120, size: 36 };

  it("evaluates assertions like devall-http", () => {
    expect(checkAssertion({ assert: "status", op: "eq", value: "200" }, ctx)).toBeNull();
    expect(checkAssertion({ assert: "status", op: "lt", value: "300" }, ctx)).toBeNull();
    expect(checkAssertion({ assert: "responseTime", op: "lt", value: "100" }, ctx)).toMatch(/response time lt 100/);
    expect(checkAssertion({ assert: "header", name: "X-A", op: "eq", value: "1" }, ctx)).toBeNull();
    expect(checkAssertion({ assert: "header", name: "x-missing", op: "exists" }, ctx)).toMatch(/got nothing/);
    expect(checkAssertion({ assert: "contentType", op: "contains", value: "json" }, ctx)).toBeNull();
    expect(checkAssertion({ assert: "jsonPath", path: "$.items", op: "contains", value: "2" }, ctx)).toBeNull();
    expect(checkAssertion({ assert: "jsonPath", path: "$.ok", op: "isType", value: "boolean" }, ctx)).toBeNull();
    expect(checkAssertion({ assert: "jsonPath", path: "$.name", op: "oneOf", value: "x, y" }, ctx)).toBeNull();
    expect(checkAssertion({ assert: "body", op: "matches", value: "items" }, ctx)).toBeNull();
    expect(checkAssertion({ assert: "size", op: "gt", value: "1000" }, ctx)).toMatch(/size gt 1000/);
  });

  it("substitutes uploaded secrets and never leaks them in failures", async () => {
    const seen: Request[] = [];
    const fetcher = async (r: Request) => {
      seen.push(r);
      return new Response(`token was ${r.headers.get("authorization")}`, { status: 401 });
    };
    const r: MonitorRequest = { name: "me", method: "GET", url: "https://api.example.test/me?k={{secret.KEY}}", headers: [["authorization", "Bearer {{secret.TOKEN}}"]], assertions: [{ assert: "body", op: "contains", value: "welcome" }] };
    const res = await runCheck(r, { TOKEN: "tok-123456", KEY: "key-987654" }, fetcher);
    expect(seen[0]!.url).toBe("https://api.example.test/me?k=key-987654");
    expect(seen[0]!.headers.get("authorization")).toBe("Bearer tok-123456");
    expect(res.ok).toBe(false);
    expect(res.url).toBe(r.url);
    expect(JSON.stringify(res)).not.toContain("tok-123456");
    const missing = await runCheck(r, { TOKEN: "tok-123456" }, fetcher);
    expect(missing.error).toMatch(/KEY not uploaded/);
  });

  it("encrypts secrets per account", async () => {
    const k1 = await accountKey("master-key-0123456789", "uid-1");
    const k2 = await accountKey("master-key-0123456789", "uid-2");
    const enc = await encryptSecret(k1, "s3cret", "mon:TOKEN");
    expect(enc.ct).not.toContain("s3cret");
    expect(await decryptSecret(k1, enc, "mon:TOKEN")).toBe("s3cret");
    await expect(decryptSecret(k2, enc, "mon:TOKEN")).rejects.toThrow();
    await expect(decryptSecret(k1, enc, "other:TOKEN")).rejects.toThrow();
  });

  it("validates plans and schedules", () => {
    const base = { name: "api", intervalMinutes: 60, requests: [{ name: "a", method: "GET", url: "https://x.test/", headers: [], assertions: [] }] };
    expect(validateMonitor(base, "free")).toBeNull();
    expect(validateMonitor({ ...base, intervalMinutes: 5 }, "free")).toMatch(/every 60 minutes/);
    // Plain-http alert webhooks only on self-hosted relays (a listener on the operator's network).
    expect(validateMonitor({ ...base, alerts: { webhookUrl: "http://10.0.0.5/hook" } }, "supporter")).toMatch(/https/);
    expect(validateMonitor({ ...base, alerts: { webhookUrl: "http://10.0.0.5/hook" } }, "selfhost")).toBeNull();
    expect(validateMonitor({ ...base, alerts: { webhookUrl: "ftp://10.0.0.5/hook" } }, "selfhost")).toMatch(/http/);
    expect(validateMonitor({ ...base, intervalMinutes: 5 }, "supporter")).toBeNull();
    expect(validateMonitor({ ...base, intervalMinutes: 7 }, "supporter")).toMatch(/one of/);
    expect(validateMonitor({ ...base, alerts: { slackWebhookUrl: "https://evil.test/hook" } }, "free")).toMatch(/Slack/);
    expect(validateMonitor({ ...base, requests: [{ ...base.requests[0]!, url: "ftp://x" }] }, "free")).toMatch(/http/);
    expect(isDue(60, 60)).toBe(true);
    expect(isDue(65, 60)).toBe(false);
    expect(isDue(65, 5)).toBe(true);
    expect(isDue(75, 15)).toBe(true);
  });
});

describe("custom hosts and snapshots", () => {
  it("parses CUSTOM_HOSTS in both forms", () => {
    expect([...parseCustomHosts('{"API.Example.com":"box","bad":"x","ok.example.com":"no--no"}')]).toEqual([["api.example.com", "box"]]);
    expect([...parseCustomHosts("a.example.com=one, b.example.com = two-x\nnope")]).toEqual([
      ["a.example.com", "one"],
      ["b.example.com", "two-x"],
    ]);
    expect(isValidHostname("xn--bcher-kva.example")).toBe(true);
    expect(isValidHostname("1.2.3.4")).toBe(false);
    expect(isValidHostname("a_b.example.com")).toBe(false);
  });

  it("validates snapshot uploads", () => {
    expect(validateSnapshot({ version: 1, title: "t", request: { method: "GET", url: "https://x", headers: [["a", "b"]] } })).toBeNull();
    expect(validateSnapshot({ version: 2 })).toMatch(/version/);
    expect(validateSnapshot({ version: 1, title: "t", request: { method: "GET", url: "x", headers: [["a"]] } })).toMatch(/pairs/);
  });
});
