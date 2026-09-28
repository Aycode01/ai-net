/**
 * ai-net backend server entry point.
 *
 * Initializes all agents and starts the HTTP/WebSocket server.
 */

import { createApp } from "./api/app";
import { initializeAgents, globalAgentRegistry } from "./agents";
import { startAgentSync, stopAgentSync } from "./registry/sync";
import { loadConfig } from "./config";
import { AgentCleanupService } from "./services/agentCleanup";
import { createAgentDb, getAgentDb, closeAgentDb } from "./db/agents";
import { closeDb } from "./db/index";
import { closeAuthDb } from "./db/auth";
import { closeTaskDb, getTaskDb, createTaskDb } from "./db/tasks";
import { closeJobDb } from "./queue";
import { closeEventStore, getEventStore } from "./events/eventStore";
import { createDefaultReconciliationService, closeReconciliationDb } from "./services/reconciliation";
import { DbMaintenanceService, defaultMaintenanceDatabases } from "./services/dbMaintenance";
import { ErrorRegistryMaintenanceService } from "./services/errorRegistryMaintenance";
import { EventRetentionService } from "./services/eventRetention";
import { createLogger } from "./utils/logger";
import { redactedConfigSnapshot } from "./config";
import { getDefaultIdempotencyStore, resetDefaultIdempotencyStore } from "./services/idempotency";

async function main() {
  const logger = createLogger({ module: "server" });

  try {
    // ── Validate env config at startup ──────────────────────────────────────────
    const config = loadConfig();
    logger.info({ config: redactedConfigSnapshot(config) }, "starting server");

    // Start agent sync
    startAgentSync();

    // Initialize all agents and register them
    logger.info("initializing agents");
    await initializeAgents();

    // Start agent cleanup service
    const cleanupService = new AgentCleanupService();
    cleanupService.start();

    // Start payment reconciliation. Issue #496 raises the default cadence from
    // once a day to every RECONCILIATION_INTERVAL_MS (60 s) so on-chain/off-chain
    // payment drift is detected and remediated within a minute rather than being
    // found at the end of the day.
    const reconciliationService = createDefaultReconciliationService();
    reconciliationService.start(config.RECONCILIATION_INTERVAL_MS);

    // Start idempotency key cleanup so the idempotency_keys table stays
    // bounded in production (Issue #657).  The store is initialised here with
    // the validated config so it uses the correct file-backed database and
    // honours IDEMPOTENCY_TTL_MS / IDEMPOTENCY_CLEANUP_MS from the env.
    const idempotencyStore = getDefaultIdempotencyStore(config);
    idempotencyStore.startCleanup();

    // Start SQLite maintenance (WAL checkpoint, vacuum, backup)
    const maintenanceService = new DbMaintenanceService(defaultMaintenanceDatabases(), {
      intervalMs: config.DB_MAINTENANCE_INTERVAL_MS,
      vacuumThreshold: config.DB_MAINTENANCE_VACUUM_THRESHOLD,
      backupDir: config.DB_BACKUP_DIR,
      backupRetentionCount: config.DB_BACKUP_RETENTION_COUNT,
    });
    maintenanceService.start();

    // Start error-registry maintenance (expiry sweep + per-agent cap)
    const errorRegistryMaintenance = new ErrorRegistryMaintenanceService({
      intervalMs: config.ERROR_REGISTRY_MAINTENANCE_INTERVAL_MS,
      capPerAgent: config.ERROR_REGISTRY_CAP_PER_AGENT,
    });
    errorRegistryMaintenance.start();

    // Open the file-backed event store and start event retention/compaction
    // so the live task_events table stays bounded (issue #383).
    const eventStore = getEventStore();
    const eventRetention = new EventRetentionService({
      eventStore,
      intervalMs: config.EVENT_COMPACTION_INTERVAL_MS,
      retentionDays: config.EVENT_RETENTION_DAYS,
      batchTasks: config.EVENT_COMPACTION_BATCH_TASKS,
      enabled: config.EVENT_COMPACTION_ENABLED,
    });
    eventRetention.start();

    // Create and start the server
    const { httpServer, close } = createApp({
      eventStore,
      jobWorkerStopTimeoutMs: config.GRACEFUL_SHUTDOWN_TIMEOUT * 1000,
    });

    const port = config.PORT;

    httpServer.listen(port, () => {
      logger.info({ port, env: config.NODE_ENV }, "server listening");
    });

    // ── Graceful shutdown ──────────────────────────────────────────────────────
    setupGracefulShutdown(httpServer, close, config, {
      cleanupService,
      reconciliationService,
      maintenanceService,
      errorRegistryMaintenance,
        eventRetention,
      globalAgentRegistry,
      idempotencyStore,
    });

  } catch (error) {
    logger.error({ err: error }, "failed to start server");
    process.exit(1);
  }
}

export interface GracefulShutdownExtras {
  cleanupService?: { stop(): void };
  reconciliationService?: { stop(): void };
  maintenanceService?: { stop(): void };
  errorRegistryMaintenance?: { stop(): void };
  eventRetention?: { stop(): void };
  globalAgentRegistry?: { shutdown(): void };
  idempotencyStore?: { stopCleanup(): void; close(): void };
}
/**
 * SIGTERM/SIGINT handler: stop accepting new work, drain in-flight jobs and
 * the WebSocket stream, flush the event store, close every database
 * connection, then exit 0 — or force-exit 1 if any of that takes longer
 * than `config.GRACEFUL_SHUTDOWN_TIMEOUT` seconds.
 *
 * In-flight tasks are drained (via `closeApp`, which awaits the job
 * worker's stop()) rather than force-failed: anything still running when
 * the drain window elapses stays "active" in the job store and is resumed
 * by the next `JobWorker.start()` (`recoverIncompleteJobs()` resets it to
 * "pending" for retry) — see `docs/e2e-testing.md` and
 * `tests/shutdown.test.ts` for the restart-mid-stream scenario.
 */
export function setupGracefulShutdown(
  httpServer: any,
  closeApp: (callback?: () => void) => void,
  config: { GRACEFUL_SHUTDOWN_TIMEOUT?: number },
  extras: GracefulShutdownExtras = {},
) {
  const logger = createLogger({ module: "shutdown" });
  let isShuttingDown = false;

  const shutdown = async (signal: string) => {
    if (isShuttingDown) return;
    isShuttingDown = true;

    logger.info({ signal }, "starting graceful shutdown sequence");

    const timeoutDuration = (config.GRACEFUL_SHUTDOWN_TIMEOUT ?? 30) * 1000;
    const forcedTimeout = setTimeout(() => {
      logger.error({ signal, timeoutSeconds: timeoutDuration / 1000 }, "force-killing timed out shutdown");
      process.exit(1);
    }, timeoutDuration);

    try {
      logger.info("closing http/ws server");
      await new Promise<void>((resolve) => {
        closeApp(() => {
          logger.info("http/ws server closed");
          resolve();
        });
      });

      logger.info("stopping background services");
      stopAgentSync();
      extras.cleanupService?.stop();
      extras.reconciliationService?.stop();
      extras.maintenanceService?.stop();
      extras.errorRegistryMaintenance?.stop();
      extras.globalAgentRegistry?.shutdown();
      extras.idempotencyStore?.stopCleanup();

      logger.info("failing running tasks");
      try {
        const taskDb = createTaskDb(getTaskDb());
        taskDb.failRunningTasks();
      } catch (err) {
        logger.error({ err }, "failed to mark tasks as failed during shutdown");
      }

      logger.info("marking online agents offline");
      try {
        const agentDb = createAgentDb(getAgentDb());
        agentDb.markAllOffline();
      } catch (err) {
        logger.error({ err }, "failed to mark agents offline during shutdown");
      }

      logger.info("closing database connections");
      closeDb();
      closeAgentDb();
      closeTaskDb();
      closeJobDb();
      closeAuthDb();
      closeEventStore();
      closeReconciliationDb();
      resetDefaultIdempotencyStore();

      logger.info({ signal }, "graceful shutdown complete");
      clearTimeout(forcedTimeout);
      process.exit(0);
    } catch (error) {
      logger.error({ err: error }, "error during graceful shutdown");
      process.exit(1);
    }
  };

  process.on("SIGTERM", () => shutdown("SIGTERM"));
  process.on("SIGINT", () => shutdown("SIGINT"));

  return shutdown;
}

if (require.main === module) {
  main();
}
