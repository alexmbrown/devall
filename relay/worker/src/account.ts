// AccountDO — one per user id. Strongly consistent per-account state the tunnels share:
// monthly bandwidth ledger, live tunnel registry (concurrency limit), and inbox ids per project.

import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import type { LoadTarget } from "./load";
import { randomInboxId } from "./names";
import { addToLedger, dayKey, monthPeriod, type Ledger } from "./quota";
import type { UsagePayload } from "./protocol";

export interface TunnelEntry {
  sub: string;
  connId: string;
  since: number;
}

export type RegisterResult =
  | { ok: true; usage: UsagePayload }
  | { ok: false; code: "too_many_tunnels"; message: string; active: string[] };

export interface MockEntry {
  name: string;
  createdAt: number;
}

export interface ShareEntry {
  id: string;
  title: string;
  createdAt: number;
  expiresAt: number;
  bytes: number;
}

export interface MonitorEntry {
  id: string;
  name: string;
  createdAt: number;
}

/** The account's cloud load run in progress (one at a time) and today's run count. */
interface LoadRuns {
  day: string;
  count: number;
  current: { runId: string; startedAt: number; expiresAt: number; stop: boolean } | null;
}

export type InboxResult = { ok: true; inboxId: string; created: boolean } | { ok: false; code: "too_many_inboxes"; message: string };

export interface AccountSummary {
  usage: UsagePayload;
  tunnels: TunnelEntry[];
  inboxes: { projectKey: string; inboxId: string }[];
}

/** Key used for the free tier's single inbox (and when no project key is given). */
export const DEFAULT_INBOX_KEY = "_default";

export function usagePayload(l: Ledger | undefined, now = Date.now()): UsagePayload {
  const p = monthPeriod(now);
  const cur = l && l.period === p.key ? l : { bytes: 0, requests: 0 };
  return { bandwidthBytes: cur.bytes, requests: cur.requests, periodStart: new Date(p.start).toISOString(), periodEnd: new Date(p.end).toISOString() };
}

export class AccountDO extends DurableObject<Env> {
  private async ledger(): Promise<Ledger | undefined> {
    return this.ctx.storage.get<Ledger>("ledger");
  }

  private async tunnels(): Promise<Record<string, TunnelEntry>> {
    return (await this.ctx.storage.get<Record<string, TunnelEntry>>("tunnels")) ?? {};
  }

  /** Registers a live tunnel; enforces the plan's concurrent-tunnel limit. */
  async registerTunnel(args: { sub: string; connId: string; maxTunnels: number | null }): Promise<RegisterResult> {
    const tunnels = await this.tunnels();
    if (!tunnels[args.sub] && args.maxTunnels !== null && Object.keys(tunnels).length >= args.maxTunnels) {
      // Prune entries whose sockets are gone (a tunnel DO that crashed never reported its close).
      for (const t of Object.values(tunnels)) {
        let live = false;
        try {
          live = await this.env.TUNNEL.get(this.env.TUNNEL.idFromName(t.sub)).isLive(t.connId);
        } catch {
          live = false;
        }
        if (!live) delete tunnels[t.sub];
      }
      if (Object.keys(tunnels).length >= args.maxTunnels) {
        await this.ctx.storage.put("tunnels", tunnels);
        return {
          ok: false,
          code: "too_many_tunnels",
          message: `Your plan allows ${args.maxTunnels} tunnel${args.maxTunnels === 1 ? "" : "s"} at a time. Stop one first (${Object.keys(tunnels).join(", ")}).`,
          active: Object.keys(tunnels),
        };
      }
    }
    tunnels[args.sub] = { sub: args.sub, connId: args.connId, since: Date.now() };
    await this.ctx.storage.put("tunnels", tunnels);
    return { ok: true, usage: usagePayload(await this.ledger()) };
  }

  async tunnelEnded(sub: string, connId: string): Promise<void> {
    const tunnels = await this.tunnels();
    if (tunnels[sub]?.connId === connId) {
      delete tunnels[sub];
      await this.ctx.storage.put("tunnels", tunnels);
    }
  }

  async addUsage(bytes: number, requests: number): Promise<UsagePayload> {
    const next = addToLedger(await this.ledger(), bytes, requests);
    await this.ctx.storage.put("ledger", next);
    return usagePayload(next);
  }

  async getUsage(): Promise<UsagePayload> {
    return usagePayload(await this.ledger());
  }

  /** Returns (creating on first use) the inbox id for a project key. */
  async allocateInbox(projectKey: string, maxInboxes: number | null): Promise<InboxResult> {
    const inboxes = (await this.ctx.storage.get<Record<string, string>>("inboxes")) ?? {};
    const existing = inboxes[projectKey];
    if (existing) return { ok: true, inboxId: existing, created: false };
    if (maxInboxes !== null && Object.keys(inboxes).length >= maxInboxes) {
      return {
        ok: false,
        code: "too_many_inboxes",
        message: `Your plan allows ${maxInboxes} hosted inbox${maxInboxes === 1 ? "" : "es"}.`,
      };
    }
    const inboxId = randomInboxId();
    inboxes[projectKey] = inboxId;
    await this.ctx.storage.put("inboxes", inboxes);
    return { ok: true, inboxId, created: true };
  }

  // ---------- hosted mocks / shares / monitors (counts per plan) ----------

  async registerMock(name: string, max: number | null): Promise<{ ok: true } | { ok: false; message: string }> {
    const mocks = (await this.ctx.storage.get<Record<string, MockEntry>>("mocks")) ?? {};
    if (!mocks[name] && max !== null && Object.keys(mocks).length >= max) {
      return { ok: false, message: `Your plan allows ${max} hosted mock${max === 1 ? "" : "s"} (${Object.keys(mocks).join(", ")}). Unpublish one first.` };
    }
    mocks[name] = { name, createdAt: mocks[name]?.createdAt ?? Date.now() };
    await this.ctx.storage.put("mocks", mocks);
    return { ok: true };
  }

  async removeMock(name: string): Promise<void> {
    const mocks = (await this.ctx.storage.get<Record<string, MockEntry>>("mocks")) ?? {};
    if (mocks[name]) {
      delete mocks[name];
      await this.ctx.storage.put("mocks", mocks);
    }
  }

  async listMocks(): Promise<MockEntry[]> {
    return Object.values((await this.ctx.storage.get<Record<string, MockEntry>>("mocks")) ?? {});
  }

  private async liveShares(now = Date.now()): Promise<Record<string, ShareEntry>> {
    const shares = (await this.ctx.storage.get<Record<string, ShareEntry>>("shares")) ?? {};
    let pruned = false;
    for (const [id, e] of Object.entries(shares)) {
      if (e.expiresAt <= now) {
        delete shares[id];
        pruned = true;
      }
    }
    if (pruned) await this.ctx.storage.put("shares", shares);
    return shares;
  }

  async canShare(max: number | null): Promise<boolean> {
    return max === null || Object.keys(await this.liveShares()).length < max;
  }

  async addShare(e: ShareEntry): Promise<void> {
    const shares = await this.liveShares();
    shares[e.id] = e;
    await this.ctx.storage.put("shares", shares);
  }

  async removeShare(id: string): Promise<boolean> {
    const shares = await this.liveShares();
    if (!shares[id]) return false;
    delete shares[id];
    await this.ctx.storage.put("shares", shares);
    return true;
  }

  async listShares(): Promise<ShareEntry[]> {
    return Object.values(await this.liveShares()).sort((a, b) => b.createdAt - a.createdAt);
  }

  async addMonitor(id: string, name: string, max: number | null): Promise<{ ok: true } | { ok: false; message: string }> {
    const mons = (await this.ctx.storage.get<Record<string, MonitorEntry>>("monitors")) ?? {};
    if (!mons[id] && max !== null && Object.keys(mons).length >= max) {
      return { ok: false, message: max === 0 ? "Scheduled monitors aren't included in your plan (or self-host the relay for free)." : `Your plan allows ${max} monitor${max === 1 ? "" : "s"}.` };
    }
    mons[id] = { id, name, createdAt: mons[id]?.createdAt ?? Date.now() };
    await this.ctx.storage.put("monitors", mons);
    return { ok: true };
  }

  async removeMonitor(id: string): Promise<boolean> {
    const mons = (await this.ctx.storage.get<Record<string, MonitorEntry>>("monitors")) ?? {};
    if (!mons[id]) return false;
    delete mons[id];
    await this.ctx.storage.put("monitors", mons);
    return true;
  }

  async listMonitors(): Promise<MonitorEntry[]> {
    return Object.values((await this.ctx.storage.get<Record<string, MonitorEntry>>("monitors")) ?? {}).sort((a, b) => a.createdAt - b.createdAt);
  }

  /** After a downgrade only the oldest `max` monitors keep running. */
  async monitorAllowed(id: string, max: number): Promise<boolean> {
    const list = await this.listMonitors();
    const i = list.findIndex((m) => m.id === id);
    return i >= 0 && i < max;
  }

  // ---------- cloud load regions ----------

  async loadTargets(): Promise<LoadTarget[]> {
    return Object.values((await this.ctx.storage.get<Record<string, LoadTarget>>("loadTargets")) ?? {}).sort((a, b) => a.createdAt - b.createdAt);
  }

  async putLoadTarget(t: LoadTarget, max: number): Promise<{ ok: true } | { ok: false; message: string }> {
    const all = (await this.ctx.storage.get<Record<string, LoadTarget>>("loadTargets")) ?? {};
    if (!all[t.key] && Object.keys(all).length >= max) return { ok: false, message: `Up to ${max} load-test targets per account; remove one first.` };
    all[t.key] = t;
    await this.ctx.storage.put("loadTargets", all);
    return { ok: true };
  }

  async removeLoadTarget(key: string): Promise<boolean> {
    const all = (await this.ctx.storage.get<Record<string, LoadTarget>>("loadTargets")) ?? {};
    if (!all[key]) return false;
    delete all[key];
    await this.ctx.storage.put("loadTargets", all);
    return true;
  }

  private async loadRuns(now: number): Promise<LoadRuns> {
    const r = (await this.ctx.storage.get<LoadRuns>("loadRuns")) ?? { day: dayKey(now), count: 0, current: null };
    if (r.day !== dayKey(now)) {
      r.day = dayKey(now);
      r.count = 0;
    }
    if (r.current && r.current.expiresAt <= now) r.current = null;
    return r;
  }

  async loadRunsToday(): Promise<number> {
    return (await this.loadRuns(Date.now())).count;
  }

  /** Claims the account's single run slot and one of today's runs. */
  async loadRunBegin(runId: string, maxPerDay: number | null, ttlMs: number): Promise<{ ok: true } | { ok: false; code: "load_busy" | "load_quota"; message: string }> {
    const now = Date.now();
    const r = await this.loadRuns(now);
    if (r.current) return { ok: false, code: "load_busy", message: "Another cloud load run of this account is in progress; wait for it or stop it." };
    if (maxPerDay !== null && r.count >= maxPerDay) return { ok: false, code: "load_quota", message: `Your plan includes ${maxPerDay} cloud load runs per day (UTC); try again tomorrow.` };
    r.count += 1;
    r.current = { runId, startedAt: now, expiresAt: now + ttlMs, stop: false };
    await this.ctx.storage.put("loadRuns", r);
    return { ok: true };
  }

  async loadRunEnd(runId: string): Promise<void> {
    const r = await this.loadRuns(Date.now());
    if (r.current?.runId === runId) {
      r.current = null;
      await this.ctx.storage.put("loadRuns", r);
    }
  }

  async loadRunStop(runId: string): Promise<boolean> {
    const r = await this.loadRuns(Date.now());
    if (r.current?.runId !== runId) return false;
    r.current.stop = true;
    await this.ctx.storage.put("loadRuns", r);
    return true;
  }

  async loadRunStopRequested(runId: string): Promise<boolean> {
    const r = await this.loadRuns(Date.now());
    return r.current?.runId === runId && r.current.stop;
  }

  async summary(): Promise<AccountSummary> {
    const inboxes = (await this.ctx.storage.get<Record<string, string>>("inboxes")) ?? {};
    return {
      usage: usagePayload(await this.ledger()),
      tunnels: Object.values(await this.tunnels()),
      inboxes: Object.entries(inboxes).map(([projectKey, inboxId]) => ({ projectKey, inboxId })),
    };
  }
}
