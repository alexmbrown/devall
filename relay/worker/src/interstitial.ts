// Free-tier anti-phishing interstitial (ngrok/zrok style): only top-level browser navigations to
// free tunnels see it once a week; webhooks, APIs, curl and paid tunnels never do.

export const CONTINUE_COOKIE = "devall_continue";
export const CONTINUE_PATH = "/__devall/continue";
export const SKIP_HEADER = "devall-skip-browser-warning";
export const CONTINUE_MAX_AGE = 7 * 24 * 3600;

export function hasContinueCookie(cookieHeader: string | null): boolean {
  if (!cookieHeader) return false;
  return cookieHeader.split(";").some((c) => c.trim() === `${CONTINUE_COOKIE}=1`);
}

/** Whether this request is a browser page navigation that should see the warning first. */
export function shouldShowInterstitial(request: Request, enabled: boolean): boolean {
  if (!enabled) return false;
  if (request.method !== "GET") return false;
  const h = request.headers;
  if (h.has(SKIP_HEADER)) return false;
  if ((h.get("upgrade") ?? "").toLowerCase() === "websocket") return false;
  const accept = h.get("accept") ?? "";
  if (!accept.toLowerCase().includes("text/html")) return false;
  const ua = h.get("user-agent") ?? "";
  if (!ua.startsWith("Mozilla/")) return false;
  const dest = h.get("sec-fetch-dest");
  if (dest && dest !== "document") return false;
  const mode = h.get("sec-fetch-mode");
  if (mode && mode !== "navigate") return false;
  if (hasContinueCookie(h.get("cookie"))) return false;
  try {
    if (new URL(request.url).pathname === CONTINUE_PATH) return false;
  } catch {
    return false;
  }
  return true;
}

/** Only same-origin absolute paths are allowed as the continue target (no open redirects). */
export function safeContinueTarget(to: string | null): string {
  if (!to || !to.startsWith("/") || to.startsWith("//") || to.startsWith("/\\") || /[\r\n]/.test(to)) return "/";
  return to;
}

/** `GET /__devall/continue?to=/path` → sets the cookie and redirects back. */
export function continueResponse(url: URL): Response {
  const to = safeContinueTarget(url.searchParams.get("to"));
  return new Response(null, {
    status: 303,
    headers: {
      location: to,
      "set-cookie": `${CONTINUE_COOKIE}=1; Path=/; Max-Age=${CONTINUE_MAX_AGE}; Secure; HttpOnly; SameSite=Lax`,
      "cache-control": "no-store",
      "x-robots-tag": "noindex, nofollow, noarchive",
    },
  });
}
