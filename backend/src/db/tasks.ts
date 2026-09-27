import Database from "better-sqlite3";
import path from "path";
import type { Task, TaskStatus } from "../types/task";
import type { QualityScoreRecord } from "../services/qualityScorer.types";
import { createLogger } from "../utils/logger";
import { migrateToLatest } from "./migrator";
import { createPool, type SqlitePool } from "./pool";
import { decodeCursor, encodeCursor, type CursorPage } from "./cursor";

const logger = createLogger({ component: "task-db" });
const MIGRATIONS_DIR = path.join(__dirname, "migrations", "tasks");

let _taskPool: SqlitePool | null = null;

/** Lazily open (or reopen) the pooled task database. */
export function getTaskPool(dbPath?: string): SqlitePool {
  if (!_taskPool || _taskPool.closed) {
    const filePath = dbPath ?? path.join(process.cwd(), "tasks.db");
    _taskPool = createPool({
      filePath,
      min: 1,
      max: 4,
      acquireTimeoutMs: 5_000,
      onCreate: (db) => {
        try {
          (db as any).on("error", (err: Error) => {
            logger.error({ err }, "task database error");
          });
        } catch {
          // error events are emitted from node EventEmitter support in runtime
        }
        migrateToLatest(db, MIGRATIONS_DIR);
      },
    });
  }
  return _taskPool;
}

/**
 * The writer connection, for the synchronous `createTaskDb` API.
 *
 * Kept so existing callers work unchanged; new code should prefer
 * `getTaskPool().read(...)` so reads are spread across the pool.
 */
export function getTaskDb(dbPath?: string): Database.Database {
  return getTaskPool(dbPath).writer;
}

/** The task pool if one is open, else null. Used by the metrics endpoint. */
export function currentTaskPool(): SqlitePool | null {
  return _taskPool && !_taskPool.closed ? _taskPool : null;
}

export function closeTaskDb(): void {
  void _taskPool?.close();
  _taskPool = null;
}

export interface TaskEvent {
  type: string;
  taskId: string;
  nodeId?: string;
  payload?: unknown;
  /** ISO-8601 timestamp — stored in the `occurred_at` column (schema A). */
  timestamp: string;
}

export interface TaskListOptions {
  status?: string;
  q?: string;
  sort?: "createdAt:asc" | "createdAt:desc";
  /** ISO timestamp — only return tasks created after this point. */
  createdAfter?: string;
}

export interface TaskCursorOptions {
  /** Opaque cursor from a previous page's nextCursor field. */
  cursor?: string;
  /** Max items per page (1–100, default 20). */
  limit?: number;
  status?: string;
  q?: string;
  sort?: "createdAt:asc" | "createdAt:desc";
}

export interface TaskDb {
  insert(task: Task): void;
  findById(id: string): Task | undefined;
  list(
    walletPublicKey: string,
    page: number,
    pageSize: number,
    options?: TaskListOptions,
  ): { tasks: Task[]; total: number };
  /**
   * Cursor-based list — stable under concurrent writes.
   * Default keyset: (createdAt DESC, id DESC).
   */
  listCursor(
    walletPublicKey: string,
    options?: TaskCursorOptions,
  ): CursorPage<Task>;
  updateStatus(id: string, status: TaskStatus): void;
  updateDagJson(id: string, dagJson: string): void;
  insertEvent(event: TaskEvent): void;
  getEventHistory(taskId: string): TaskEvent[];
  failRunningTasks(): void;
  insertQualityScore(record: QualityScoreRecord): void;
  listQualityScores(agentId?: string, limit?: number, cursor?: number): QualityScoreRecord[];
}

export function createTaskDb(db: Database.Database): TaskDb {
  return {
    insert(task: Task): void {
      db.prepare(
        `
        INSERT INTO tasks (id, prompt, walletPublicKey, status, dagJson, createdAt, updatedAt)
        VALUES (@id, @prompt, @walletPublicKey, @status, @dagJson, @createdAt, @updatedAt)
      `,
      ).run({
        ...task,
        dagJson: JSON.stringify(task.dag),
      });
    },

    findById(id: string): Task | undefined {
      const row = db.prepare("SELECT * FROM tasks WHERE id = ?").get(id) as any;
      if (!row) return undefined;
      return {
        ...row,
        dag: JSON.parse(row.dagJson),
      };
    },

    list(
      walletPublicKey: string,
      page: number,
      pageSize: number,
      options: TaskListOptions = {},
    ) {
      const offset = (page - 1) * pageSize;
      const conditions: string[] = ["walletPublicKey = ?"];
      const params: unknown[] = [walletPublicKey];

      if (options.status) {
        conditions.push("status = ?");
        params.push(options.status);
      }
      if (options.q) {
        conditions.push("prompt LIKE ?");
        params.push(`%${options.q}%`);
      }
      if (options.createdAfter) {
        conditions.push("createdAt > ?");
        params.push(options.createdAfter);
      }

      const whereClause = conditions.join(" AND ");
      const sortOrder = options.sort === "createdAt:asc" ? "ASC" : "DESC";

      const rows = db
        .prepare(
          `SELECT * FROM tasks WHERE ${whereClause} ORDER BY createdAt ${sortOrder} LIMIT ? OFFSET ?`,
        )
        .all(...params, pageSize, offset) as any[];

      const tasks: Task[] = rows.map((row) => ({
        ...row,
        dag: JSON.parse(row.dagJson),
      }));

      const countRow = db
        .prepare(`SELECT COUNT(*) as total FROM tasks WHERE ${whereClause}`)
        .get(...params) as { total?: number } | undefined;
      const total = countRow?.total ?? 0;

      return { tasks, total };
    },

    listCursor(
      walletPublicKey: string,
      options: TaskCursorOptions = {},
    ): CursorPage<Task> {
      const limit = Math.min(Math.max(options.limit ?? 20, 1), 100);
      const sortOrder = options.sort === "createdAt:asc" ? "ASC" : "DESC";
      // Keyset comparator flips based on sort direction
      const keyOp = sortOrder === "DESC" ? "<" : ">";

      const conditions: string[] = ["walletPublicKey = ?"];
      const params: unknown[] = [walletPublicKey];

      if (options.status) {
        conditions.push("status = ?");
        params.push(options.status);
      }
      if (options.q) {
        conditions.push("prompt LIKE ?");
        params.push(`%${options.q}%`);
      }

      let cursorCondition = "";
      const cursorParams: unknown[] = [];

      if (options.cursor) {
        const payload = decodeCursor(options.cursor);
        if (payload?.createdAt && payload?.id) {
          // Compound keyset prevents instability when timestamps collide
          cursorCondition = `AND (createdAt ${keyOp} ? OR (createdAt = ? AND id ${keyOp} ?))`;
          cursorParams.push(payload.createdAt, payload.createdAt, payload.id);
        }
      }

      const whereClause = conditions.join(" AND ");
      // Fetch limit+1 to detect a next page without a COUNT query
      const rows = db
        .prepare(
          `SELECT * FROM tasks
           WHERE ${whereClause} ${cursorCondition}
           ORDER BY createdAt ${sortOrder}, id ${sortOrder}
           LIMIT ?`,
        )
        .all(...params, ...cursorParams, limit + 1) as any[];

      const hasMore = rows.length > limit;
      const pageRows = hasMore ? rows.slice(0, limit) : rows;

      const tasks: Task[] = pageRows.map((row) => ({
        ...row,
        dag: JSON.parse(row.dagJson),
      }));

      const result: CursorPage<Task> = { items: tasks };
      if (hasMore) {
        const last = pageRows[pageRows.length - 1];
        result.nextCursor = encodeCursor({ createdAt: last.createdAt, id: last.id });
      }
      return result;
    },

    updateStatus(id: string, status: TaskStatus): void {
      db.prepare("UPDATE tasks SET status = ?, updatedAt = ? WHERE id = ?").run(
        status,
        new Date().toISOString(),
        id,
      );
    },

    updateDagJson(id: string, dagJson: string): void {
      db.prepare(
        "UPDATE tasks SET dagJson = ?, updatedAt = ? WHERE id = ?",
      ).run(dagJson, new Date().toISOString(), id);
    },

    insertEvent(event: TaskEvent): void {
      // Assign a per-task sequence number by looking up the current max.
      // This is a simple approach suitable for the legacy TaskDb path; the
      // EventBus / EventStore path uses its own atomic counter.
      const row = db
        .prepare(
          'SELECT COALESCE(MAX(task_seq), -1) AS max_seq FROM task_events WHERE task_id = ?',
        )
        .get(event.taskId) as { max_seq: number };
      const nextSeq = (row?.max_seq ?? -1) + 1;

      db.prepare(
        `
        INSERT INTO task_events (task_seq, version, type, task_id, node_id, occurred_at, payload)
        VALUES (@task_seq, @version, @type, @task_id, @node_id, @occurred_at, @payload)
      `,
      ).run({
        task_seq: nextSeq,
        version: 1,
        type: event.type,
        task_id: event.taskId,
        node_id: event.nodeId ?? null,
        occurred_at: event.timestamp,
        payload:
          event.payload !== undefined ? JSON.stringify(event.payload) : null,
      });
    },

    getEventHistory(taskId: string): TaskEvent[] {
      const rows = db
        .prepare(
          'SELECT * FROM task_events WHERE task_id = ? ORDER BY task_seq ASC',
        )
        .all(taskId) as Array<{
        task_id: string;
        type: string;
        node_id: string | null;
        payload: string | null;
        occurred_at: string;
      }>;
      return rows.map((r) => ({
        taskId: r.task_id,
        type: r.type,
        nodeId: r.node_id ?? undefined,
        payload: r.payload ? JSON.parse(r.payload) : undefined,
        timestamp: r.occurred_at,
      }));
    },

    failRunningTasks(): void {
      const now = new Date().toISOString();
      const runningTasks = db.prepare("SELECT * FROM tasks WHERE status = 'running'").all() as any[];
      for (const task of runningTasks) {
        let dag: any[] = [];
        try {
          dag = JSON.parse(task.dagJson);
          for (const node of dag) {
            if (node.status === 'running' || node.status === 'pending') {
              node.status = 'failed';
              node.error = 'Server shutdown';
            }
          }
        } catch (e) {
          // ignore parse error
        }
        db.prepare("UPDATE tasks SET status = 'failed', dagJson = ?, updatedAt = ? WHERE id = ?").run(
          JSON.stringify(dag),
          now,
          task.id
        );
      }
    },

    insertQualityScore(record: QualityScoreRecord): void {
      db.prepare(
        `
        INSERT INTO quality_scores (taskId, nodeId, agentId, agentType, score, completeness, relevance, format, needsReview, timestamp)
        VALUES (@taskId, @nodeId, @agentId, @agentType, @score, @completeness, @relevance, @format, @needsReview, @timestamp)
      `,
      ).run({
        taskId: record.taskId,
        nodeId: record.nodeId,
        agentId: record.agentId ?? null,
        agentType: record.agentType,
        score: record.score,
        completeness: record.completeness,
        relevance: record.relevance,
        format: record.format,
        needsReview: record.needsReview ? 1 : 0,
        timestamp: record.timestamp,
      });
    },

    listQualityScores(agentId?: string, limit: number = 500, cursor?: number): QualityScoreRecord[] {
      let boundedLimit = typeof limit === "number" && !isNaN(limit) ? Math.floor(limit) : 500;
      boundedLimit = Math.max(1, Math.min(500, boundedLimit));

      const conditions: string[] = [];
      const params: any[] = [];

      if (agentId) {
        conditions.push("agentId = ?");
        params.push(agentId);
      }

      if (cursor !== undefined && cursor !== null && !isNaN(Number(cursor))) {
        conditions.push("id < ?");
        params.push(Number(cursor));
      }

      const whereClause = conditions.length > 0 ? `WHERE ${conditions.join(" AND ")}` : "";
      const query = `SELECT * FROM quality_scores ${whereClause} ORDER BY id DESC LIMIT ?`;
      params.push(boundedLimit);

      const rows = db.prepare(query).all(...params) as Array<{
        id: number;
        taskId: string;
        nodeId: string;
        agentId: string | null;
        agentType: string;
        score: number;
        completeness: number;
        relevance: number;
        format: number;
        needsReview: number | null;
        timestamp: string;
      }>;

      return rows.map((r) => ({
        id: r.id,
        taskId: r.taskId,
        nodeId: r.nodeId,
        agentId: r.agentId ?? undefined,
        agentType: r.agentType,
        score: r.score,
        completeness: r.completeness,
        relevance: r.relevance,
        format: r.format,
        /** Default legacy NULL or non-1 needsReview to false */
        needsReview: r.needsReview === 1,
        timestamp: r.timestamp,
      }));
    },
  };
}
