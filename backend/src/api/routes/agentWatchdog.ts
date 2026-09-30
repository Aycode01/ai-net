/**
 * Agent watchdog alert feed (Issue #379).
 *
 * Backs the dashboard's "fleet health" panel: which agents are currently in
 * their heartbeat grace period, what has been evicted, and when. The acceptance
 * criterion is that alerts are *visible*, and a log line alone is not visible to
 * anyone who is not tailing logs.
 *
 * Routes (mounted under /api):
 *   GET /agent-watchdog/alerts    — recent alerts, newest first
 *   GET /agent-watchdog/quarantine — agents currently inside the grace period
 *   POST /agent-watchdog/tick     — force a detection pass (ops/admin use)
 */

import { Router, Request, Response, NextFunction } from "express";
import { createAgentDb, getAgentDb, type AgentDb } from "../../db/agents";
import { createLogger } from "../../utils/logger";
import { adminLimiter } from "../middleware/rateLimit";

const log = createLogger({ component: "agent-watchdog-routes" });

export interface AgentWatchdogRouterOptions {
  /** Run a detection pass on demand. Omitted routes still serve reads. */
  tick?: () => Promise<{ quarantined: number; evicted: number; recovered: number }>;
  db?: AgentDb;
}

export function createAgentWatchdogRouter(options: AgentWatchdogRouterOptions = {}): Router {
  const router = Router();
  const getDb = (): AgentDb => options.db ?? createAgentDb(getAgentDb());

  /**
   * GET /api/agent-watchdog/alerts
   *
   * `?unresolvedOnly=true` is the dashboard's default view: it shows what is
   * still wrong rather than burying today's problem under a week of history.
   */
  router.get("/alerts", (req: Request, res: Response, next: NextFunction): void => {
    try {
      const db = getDb();
      const limit = clampLimit(req.query.limit, 50, 200);
      const unresolvedOnly = req.query.unresolvedOnly === "true";
      const agentId = typeof req.query.agentId === "string" ? req.query.agentId : undefined;

      const alerts = db.listAlerts({ limit, agentId, unresolvedOnly });

      res.json({
        data: alerts,
        _meta: {
          version: "2.0",
          timestamp: new Date().toISOString(),
          count: alerts.length,
        },
      });
    } catch (err) {
      next(err);
    }
  });

  /**
   * GET /api/agent-watchdog/quarantine
   *
   * Agents that missed a heartbeat but are still inside the grace period. This
   * is the "unavailable but not yet evicted" set — the state that proves the
   * acceptance criterion that a stale agent stops being dispatchable
   * immediately rather than only when it is finally deleted.
   */
  router.get("/quarantine", (_req: Request, res: Response, next: NextFunction): void => {
    try {
      const db = getDb();
      const now = Date.now();
      const quarantined = db.listStaleAgents().map((agent) => {
        const staleSinceMs = db.getStaleSince(agent.id);
        const lastSeenMs = Date.parse(agent.lastSeenAt);
        return {
          agentId: agent.id,
          capabilities: agent.capabilities,
          endpoint: agent.endpoint,
          lastSeenAt: agent.lastSeenAt,
          silentForMs: Number.isFinite(lastSeenMs) ? now - lastSeenMs : null,
          quarantinedSince:
            staleSinceMs !== null ? new Date(staleSinceMs).toISOString() : null,
          reputationScore: agent.reputationScore,
        };
      });

      res.json({
        data: quarantined,
        _meta: { version: "2.0", timestamp: new Date().toISOString(), count: quarantined.length },
      });
    } catch (err) {
      next(err);
    }
  });

  /**
   * POST /api/agent-watchdog/tick
   *
   * Forces a detection pass instead of waiting for the next interval. Rate
   * limited at the admin tier: it performs deletes, and an unauthenticated
   * caller should not be able to trigger evictions on a timer.
   */
  router.post("/tick", adminLimiter.middleware, (_req: Request, res: Response, next: NextFunction): void => {
    if (!options.tick) {
      res.status(501).json({
        error: { code: "NOT_IMPLEMENTED", message: "watchdog tick is not wired" },
      });
      return;
    }

    options.tick()
      .then((stats) => {
        log.info(stats, "agent watchdog tick (manual)");
        res.json({ data: stats, _meta: { version: "2.0", timestamp: new Date().toISOString() } });
      })
      .catch(next);
  });

  return router;
}

function clampLimit(raw: unknown, fallback: number, max: number): number {
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed <= 0) return fallback;
  return Math.min(Math.floor(parsed), max);
}
