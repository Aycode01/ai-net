/**
 * Prometheus metrics export route.
 *
 * Exposes:
 *  - GET /metrics         — Prometheus text format scrape target
 *  - GET /metrics/health  — Scrape reliability and uptime diagnostics
 *  - POST /metrics/reset  — Safe metrics reset
 */

import { Router, Request, Response, NextFunction } from "express";
import { adminAuthMiddleware } from "../middleware/auth";
import { metricsService, MetricsService } from "../../services/metrics";

export interface MetricsRouterOptions {
  service?: MetricsService;
}

export function createMetricsRouter(options: MetricsRouterOptions = {}): Router {
  const router = Router();
  const service = options.service ?? metricsService;

  /**
   * @openapi
   * /metrics:
   *   get:
   *     summary: Prometheus metrics endpoint
   *     description: Exports system and domain metrics in Prometheus text format for scraping.
   *     tags: [Metrics]
   *     security: []
   *     responses:
   *       200:
   *         description: Prometheus metric families
   *         content:
   *           text/plain:
   *             schema:
   *               type: string
   */
  router.get("/", async (_req: Request, res: Response, next: NextFunction): Promise<void> => {
    try {
      const output = await service.exportPrometheusMetrics();
      res.setHeader("Content-Type", "text/plain; version=0.0.4; charset=utf-8");
      res.status(200).send(output);
    } catch (err) {
      next(err);
    }
  });

  /**
   * @openapi
   * /metrics/health:
   *   get:
   *     summary: Prometheus scrape health
   *     description: Checks telemetry scrape reliability and registration status.
   *     tags: [Metrics]
   *     security: []
   *     responses:
   *       200:
   *         description: Scrape health status
   *         content:
   *           application/json:
   *             schema:
   *               type: object
   */
  router.get("/health", (_req: Request, res: Response): void => {
    const health = service.getScrapeHealth();
    res.status(200).json(health);
  });

  /**
   * @openapi
   * /api/metrics:
   *   get:
   *     summary: Prometheus metrics endpoint (namespaced alias)
   *     description: >
   *       Alias of `GET /metrics` under the `/api` prefix, for deployments that
   *       route all application traffic through a single gateway prefix.
   *       Exports system and domain metrics in Prometheus text format.
   *     tags: [Metrics]
   *     security: []
   *     responses:
   *       200:
   *         description: Prometheus metric families
   *         content:
   *           text/plain:
   *             schema:
   *               type: string
   */

  /**
   * @openapi
   * /api/metrics/health:
   *   get:
   *     summary: Prometheus scrape health (namespaced alias)
   *     description: Alias of `GET /metrics/health`. Checks telemetry scrape reliability and registration status.
   *     tags: [Metrics]
   *     security: []
   *     responses:
   *       200:
   *         description: Scrape health status
   *         content:
   *           application/json:
   *             schema:
   *               $ref: '#/components/schemas/MetricsScrapeHealth'
   */

  /**
   * @openapi
   * /api/metrics/reset:
   *   post:
   *     summary: Reset collected Prometheus metrics (namespaced alias)
   *     description: Alias of `POST /metrics/reset`. Clears the in-memory counter and gauge buffers.
   *     tags: [Metrics, Admin]
   *     security:
   *       - adminApiKey: []
   *     responses:
   *       200:
   *         description: Metrics reset successfully
   *       401:
   *         description: Missing or invalid admin API key
   *       503:
   *         description: ADMIN_API_KEY is not configured
   */

  /**
   * @openapi
   * /metrics/reset:
   *   post:
   *     summary: Reset collected Prometheus metrics
   *     description: Clears the in-memory counter and gauge buffers. Admin-only, and fails closed when `ADMIN_API_KEY` is unset.
   *     tags: [Metrics, Admin]
   *     security:
   *       - adminApiKey: []
   *     responses:
   *       200:
   *         description: Metrics reset successfully
   *         content:
   *           application/json:
   *             schema:
   *               type: object
   *               properties:
   *                 status: { type: string, example: "ok" }
   *                 message: { type: string, example: "Metrics reset successfully" }
   *       401:
   *         description: Missing or invalid admin API key
   *       503:
   *         description: ADMIN_API_KEY is not configured
   */
  router.post("/reset", adminAuthMiddleware, (_req: Request, res: Response): void => {
    service.resetPrometheusMetrics();
    res.status(200).json({ status: "ok", message: "Metrics reset successfully" });
  });

  return router;
}

export const metricsRouter = createMetricsRouter();
