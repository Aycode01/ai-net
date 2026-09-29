import Database from "better-sqlite3";
import { existsSync, mkdirSync } from "fs";
import path from "path";
import { createLogger } from "../utils/logger";
import { migrateToLatest } from "./migrator";
import { createPool, type SqlitePool } from "./pool";

const MIGRATIONS_DIR = path.join(__dirname, "migrations", "payments");

export type PaymentStatus = "locked" | "released" | "refunded";

export interface PaymentRecord {
  taskId: string;
  nodeId: string;
  balanceId: string;
  status: PaymentStatus;
  amountStroops: bigint;
  txHash: string | null;
}

const logger = createLogger({ component: "payment-db" });

/** Database used when nothing is configured — matches config's DATABASE_URL default. */
export const DEFAULT_DB_PATH = "./data/ai-net.db";

/** `true` for `:memory:` / `file::memory:` style URIs, which need no directory. */
export function isInMemoryPath(dbPath: string): boolean {
  const value = dbPath.trim();
  return value === ":memory:" || value.startsWith("file::memory:") || /mode=memory/.test(value);
}

/**
 * Resolve the SQLite file path for the consolidated database.
 *
 * Precedence: explicit argument → `DB_PATH` → `DATABASE_URL` → default. A
 * `file:` prefix is stripped and relative paths are resolved against the
 * current working directory so `./data/ai-net.db` in `.env` means the same
 * thing regardless of where the process was started.
 *
 * @throws {Error} when the configured value looks like a non-SQLite URL
 *   (e.g. `postgresql://…`), which would otherwise create a bogus file name.
 */
export function resolveDatabasePath(override?: string): string {
  const raw = (override ?? process.env.DB_PATH ?? process.env.DATABASE_URL ?? DEFAULT_DB_PATH).trim();

  if (raw === "") {
    throw new Error("Database path is empty — set DB_PATH or DATABASE_URL.");
  }
  if (isInMemoryPath(raw)) {
    return raw;
  }
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(raw)) {
    throw new Error(
      `Unsupported database URL "${raw}". This backend stores data in SQLite — set DB_PATH ` +
        "or DATABASE_URL to a filesystem path (e.g. ./data/ai-net.db).",
    );
  }

  const withoutScheme = raw.startsWith("file:") ? raw.slice("file:".length) : raw;
  return path.resolve(withoutScheme);
}

/**
 * Open a SQLite database with the pragmas the app relies on, creating the
 * parent directory when the path points at a file. This is what lets a fresh
 * checkout run `npm run db:migrate` against a database that does not exist yet.
 */
export function openDatabase(dbPath: string): Database.Database {
  if (!isInMemoryPath(dbPath)) {
    const dir = path.dirname(dbPath);
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true });
    }
  }

  const db = new Database(dbPath);
  db.pragma("busy_timeout = 5000");
  db.pragma("journal_mode = WAL");
  return db;
}

let _pool: SqlitePool | null = null;
let _poolClosing: Promise<void> | null = null;

/**
 * Create the payments schema. Runs once, on the pool's writer connection.
 *
 * DDL only: the connection's error subscription lives in `getPaymentPool`'s
 * `onCreate`, where the writer handle is first opened. Keeping a second copy
 * here meant the very first schema application tried to subscribe to an event
 * surface the driver may not expose.
 */
function applyPaymentSchema(db: Database.Database): void {
  db.exec(`
    CREATE TABLE IF NOT EXISTS payments (
      taskId       TEXT NOT NULL,
      nodeId       TEXT NOT NULL,
      balanceId    TEXT NOT NULL,
      status       TEXT NOT NULL DEFAULT 'locked',
      amountStroops TEXT NOT NULL,
      txHash       TEXT,
      PRIMARY KEY (taskId, nodeId)
    )
  `);
}

/** The payment database's connection pool. */
export function getPaymentPool(dbPath?: string): SqlitePool {
  if (_poolClosing) throw new Error("Payment database is closing");
  if (!_pool || _pool.closed) {
    const filePath = dbPath ?? path.join(process.cwd(), "payments.db");
    _pool = createPool({
      filePath,
      min: 1,
      max: 4,
      acquireTimeoutMs: 5_000,
      onCreate: (db) => {
        try {
          // `on` is only present when the driver exposes node's EventEmitter
          // surface; without it errors surface as thrown exceptions instead, so
          // the subscription stays best-effort (same guard as db/tasks.ts).
          (db as unknown as { on: (event: string, fn: (error: Error) => void) => void }).on(
            "error",
            (error: Error) => {
              logger.error({ err: error }, "payment database error");
            },
          );
        } catch {
          // driver has no error-event support — nothing to subscribe to
        }
        applyPaymentSchema(db);
        migrateToLatest(db, MIGRATIONS_DIR);
      },
    });
    logger.info({ dbPath: filePath }, "payment database opened");
  }
  return _pool;
}

/**
 * The writer connection, for the synchronous `createPaymentDb` API.
 *
 * New code should prefer `getPaymentPool().read(...)`.
 */
export function getDb(dbPath?: string): Database.Database {
  return getPaymentPool(dbPath).writer;
}

export function closeDb(): Promise<void> {
  if (_poolClosing) return _poolClosing;
  const pool = _pool;
  if (!pool) return Promise.resolve();
  _poolClosing = pool.close().finally(() => {
    if (_pool === pool) _pool = null;
    _poolClosing = null;
  });
  return _poolClosing;
}

/** The payments pool if one is open, else null. Used by the metrics endpoint. */
export function currentPaymentPool(): SqlitePool | null {
  return _pool && !_pool.closed ? _pool : null;
}

export function paymentDbHealthCheck(): boolean {
  try {
    const db = getDb();
    db.prepare("SELECT 1").get();
    return true;
  } catch (error) {
    logger.error({ err: error }, "payment database health check failed");
    return false;
  }
}

export interface PaymentDb {
  insert(record: PaymentRecord): void;
  findByKey(taskId: string, nodeId: string): PaymentRecord | undefined;
  updateStatus(taskId: string, nodeId: string, status: PaymentStatus, txHash: string): void;
  /** All payment records — used by payment reconciliation. */
  listAll(): PaymentRecord[];
}

export function createPaymentDb(db: Database.Database): PaymentDb {
  return {
    insert(record: PaymentRecord): void {
      db.prepare(`
        INSERT INTO payments (taskId, nodeId, balanceId, status, amountStroops, txHash)
        VALUES (@taskId, @nodeId, @balanceId, @status, @amountStroops, @txHash)
      `).run({
        ...record,
        amountStroops: record.amountStroops.toString(),
        txHash: record.txHash,
      });
    },

    findByKey(taskId: string, nodeId: string): PaymentRecord | undefined {
      const row = db.prepare(
        "SELECT * FROM payments WHERE taskId = ? AND nodeId = ?"
      ).get(taskId, nodeId) as Record<string, unknown> | undefined;
      if (!row) return undefined;
      return {
        taskId: row.taskId as string,
        nodeId: row.nodeId as string,
        balanceId: row.balanceId as string,
        status: row.status as PaymentStatus,
        amountStroops: BigInt(row.amountStroops as string),
        txHash: row.txHash as string | null,
      };
    },

    updateStatus(taskId: string, nodeId: string, status: PaymentStatus, txHash: string): void {
      db.prepare(
        "UPDATE payments SET status = ?, txHash = ? WHERE taskId = ? AND nodeId = ?"
      ).run(status, txHash, taskId, nodeId);
    },

    listAll(): PaymentRecord[] {
      const rows = db.prepare("SELECT * FROM payments").all() as Array<Record<string, unknown>>;
      return rows.map((row) => ({
        taskId: row.taskId as string,
        nodeId: row.nodeId as string,
        balanceId: row.balanceId as string,
        status: row.status as PaymentStatus,
        amountStroops: BigInt(row.amountStroops as string),
        txHash: row.txHash as string | null,
      }));
    },
  };
}
