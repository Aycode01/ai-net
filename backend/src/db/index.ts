import Database from "better-sqlite3";
import { existsSync, mkdirSync } from "fs";
import path from "path";
import { createLogger } from "../utils/logger";

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

let _db: Database.Database | null = null;

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

export function getDb(dbPath?: string): Database.Database {
  if (!_db) {
    const filePath = dbPath ?? path.join(process.cwd(), "payments.db");
    _db = new Database(filePath as unknown as string);
    _db.pragma("busy_timeout = 5000");
    _db.pragma("journal_mode = WAL");
    logger.info({ dbPath: filePath }, "payment database opened");
    (_db as unknown as { on: (event: string, fn: (error: Error) => void) => void }).on(
      "error",
      (error: Error) => {
        logger.error({ err: error }, "payment database error");
      },
    );
    _db.exec(`
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
  return _db;
}

export function closeDb(): void {
  _db?.close();
  _db = null;
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
