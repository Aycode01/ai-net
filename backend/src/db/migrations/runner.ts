/**
 * Versioned, forward-only SQLite migration runner.
 *
 * Applied migrations are recorded in a `schema_migrations` bookkeeping table so
 * re-running the runner is a no-op.  Each migration is applied inside its own
 * transaction together with the bookkeeping INSERT: if the SQL fails the whole
 * unit rolls back and no later migration is attempted.
 *
 * @example
 * ```ts
 * const runner = new MigrationRunner(db, loadMigrations(dir));
 * runner.up(); // applies 0001, 0002, ...; a second call applies nothing
 * ```
 */

import type Database from "better-sqlite3";
import { createLogger } from "../../utils/logger";
import { loadMigrations, resolveMigrationsDir, type Migration } from "./loader";
import type { Logger } from "pino";

/** Row shape of the `schema_migrations` bookkeeping table. */
export interface AppliedMigration {
  id: string;
  filename: string;
  sql: string;
  appliedAt: string;
}

/** Full bookkeeping row, including the fields used for drift detection. */
export interface MigrationRecord extends AppliedMigration {
  name: string;
  checksum: string;
  downSql: string | null;
}

export type MigrationDirection = "up" | "down";

export type MigrationStatus =
  /** SQL ran and was committed. */
  | "applied"
  /** Already present in `schema_migrations`; nothing to do. */
  | "skipped"
  /** Down SQL ran and was committed. */
  | "reverted"
  /** `--dry-run`: would have run, but the database was not touched. */
  | "planned";

export interface MigrationResult {
  id: string;
  filename: string;
  direction: MigrationDirection;
  status: MigrationStatus;
  durationMs: number;
  error?: string;
}

export interface MigrationRunnerOptions {
  /** Forward-only by default: down migrations are refused unless enabled. */
  includeDownMigrations?: boolean;
  logger?: Logger;
}

/** Options for {@link MigrationRunner.up}. */
export interface UpOptions {
  /** Report what would happen without touching the database. */
  dryRun?: boolean;
  /**
   * Return a failed result instead of throwing. Defaults to `false`: a
   * database-mutating call that returns normally has applied everything.
   */
  throwOnError?: boolean;
}

/** Options for {@link MigrationRunner.down}. */
export interface DownOptions extends UpOptions {
  /** Revert migrations with a version greater than this one. */
  to?: string;
  /** Revert at most this many migrations (default: all that have down SQL). */
  steps?: number;
}

/** DDL for the bookkeeping table. Bootstrapped by the runner, not a migration. */
export const SCHEMA_MIGRATIONS_DDL = `
  CREATE TABLE IF NOT EXISTS schema_migrations (
    id         TEXT PRIMARY KEY,
    filename   TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    checksum   TEXT NOT NULL,
    sql        TEXT NOT NULL,
    down_sql   TEXT,
    applied_at TEXT NOT NULL
  )
`;

/** Base class for every failure raised by the runner. */
export class MigrationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "MigrationError";
  }
}

/** Raised when an already-applied migration file has been edited. */
export class MigrationChecksumError extends MigrationError {
  constructor(readonly migration: Migration, expected: string, actual: string) {
    super(
      `Migration "${migration.filename}" was modified after it was applied ` +
        `(recorded checksum ${expected.slice(0, 12)}…, file checksum ${actual.slice(0, 12)}…). ` +
        "Applied migrations are immutable — add a new migration instead.",
    );
    this.name = "MigrationChecksumError";
  }
}

/**
 * Raised when a migration's SQL failed. The offending unit was rolled back and
 * no later migration was attempted.
 *
 * `results` holds the outcome of every migration the run touched, so callers
 * can report what did land before the failure.
 */
export class MigrationFailedError extends MigrationError {
  constructor(readonly failed: MigrationResult, readonly results: MigrationResult[]) {
    super(`Migration "${failed.filename}" failed and was rolled back: ${failed.error ?? "unknown error"}`);
    this.name = "MigrationFailedError";
  }
}

interface MigrationRow {
  id: string;
  filename: string;
  name: string;
  checksum: string;
  sql: string;
  down_sql: string | null;
  applied_at: string;
}

export class MigrationRunner {
  private readonly db: Database.Database;
  private readonly migrations: Migration[];
  private readonly logger: Logger;
  private readonly includeDownMigrations: boolean;

  constructor(
    db: Database.Database,
    migrations: Migration[],
    options: MigrationRunnerOptions = {},
  ) {
    this.db = db;
    this.migrations = [...migrations].sort((a, b) => a.version - b.version);
    this.logger = options.logger ?? createLogger({ component: "db-migrate" });
    this.includeDownMigrations = options.includeDownMigrations ?? false;
  }

  /** Convenience factory that loads the migration files from disk. */
  static fromDirectory(
    db: Database.Database,
    options: MigrationRunnerOptions & { migrationsDir?: string } = {},
  ): MigrationRunner {
    const dir = resolveMigrationsDir(options.migrationsDir);
    return new MigrationRunner(db, loadMigrations(dir), options);
  }

  /** Create `schema_migrations` if it does not exist. Safe to call repeatedly. */
  ensureMigrationsTable(): void {
    this.db.exec(SCHEMA_MIGRATIONS_DDL);
  }

  /** Every applied migration, ordered by version. */
  listApplied(): MigrationRecord[] {
    this.ensureMigrationsTable();
    const rows = this.db
      .prepare(
        "SELECT id, filename, name, checksum, sql, down_sql, applied_at FROM schema_migrations ORDER BY id ASC",
      )
      .all() as MigrationRow[];

    return rows.map((row) => ({
      id: row.id,
      filename: row.filename,
      name: row.name,
      checksum: row.checksum,
      sql: row.sql,
      downSql: row.down_sql,
      appliedAt: row.applied_at,
    }));
  }

  /** Migrations on disk that are not yet recorded, ordered by version. */
  listPending(): Migration[] {
    const applied = new Map(this.listApplied().map((row) => [row.id, row]));
    return this.migrations.filter((migration) => !applied.has(migration.id));
  }

  /** Highest applied version, or `null` for a database that has never migrated. */
  currentVersion(): string | null {
    const applied = this.listApplied();
    return applied.length === 0 ? null : applied[applied.length - 1].id;
  }

  /**
   * Apply every pending migration in ascending version order.
   *
   * Each migration is committed together with its `schema_migrations` row, so a
   * failure rolls that migration back and stops the run — later migrations are
   * left untouched and can be retried once the cause is fixed.
   *
   * @throws {MigrationFailedError} when a migration's SQL fails, unless
   *   `throwOnError: false` is passed.
   * @throws {MigrationChecksumError} when an applied migration file was edited.
   * @throws {MigrationError} when an applied migration file is missing.
   */
  up(options: UpOptions = {}): MigrationResult[] {
    this.ensureMigrationsTable();
    this.assertNoDrift();

    const results: MigrationResult[] = [];
    for (const migration of this.listPending()) {
      if (options.dryRun) {
        results.push({
          id: migration.id,
          filename: migration.filename,
          direction: "up",
          status: "planned",
          durationMs: 0,
        });
        continue;
      }

      const startedAt = Date.now();
      try {
        this.applyOne(migration);
        results.push({
          id: migration.id,
          filename: migration.filename,
          direction: "up",
          status: "applied",
          durationMs: Date.now() - startedAt,
        });
      } catch (error) {
        // The transaction has already rolled back; report and stop so the
        // remaining migrations are not applied on top of a failed schema.
        const failure: MigrationResult = {
          id: migration.id,
          filename: migration.filename,
          direction: "up",
          status: "skipped",
          durationMs: Date.now() - startedAt,
          error: error instanceof Error ? error.message : String(error),
        };
        results.push(failure);
        this.logger.error(
          { migration: migration.filename, err: error },
          "migration failed and was rolled back; remaining migrations not applied",
        );
        if (options.throwOnError === false) {
          return results;
        }
        throw new MigrationFailedError(failure, results);
      }
    }
    return results;
  }

  /**
   * Revert applied migrations, newest first.
   *
   * Development-only affordance — the runner refuses to run unless it was
   * constructed with `includeDownMigrations: true` (the
   * `--include-down-migrations` CLI flag sets it).
   *
   * @throws {MigrationError} when down migrations are disabled or the targeted
   *   migration has no down section.
   * @throws {MigrationFailedError} when a down migration's SQL fails, unless
   *   `throwOnError: false` is passed.
   */
  down(options: DownOptions = {}): MigrationResult[] {
    if (!this.includeDownMigrations) {
      throw new MigrationError(
        "Down migrations are disabled. Re-run with --include-down-migrations to enable them.",
      );
    }

    this.ensureMigrationsTable();

    const byId = new Map(this.migrations.map((migration) => [migration.id, migration]));
    const applied = this.listApplied().filter((record) => byId.has(record.id)).reverse();

    const targetVersion = options.to === undefined ? null : Number(options.to);
    if (targetVersion !== null && Number.isNaN(targetVersion)) {
      throw new MigrationError(`Invalid --to value "${options.to}" — expected a migration version.`);
    }
    const limit = options.steps === undefined ? applied.length : Math.max(0, options.steps);
    const doomed = applied
      .filter((record) => (targetVersion === null ? true : Number(record.id) > targetVersion))
      .slice(0, limit);

    const results: MigrationResult[] = [];
    for (const record of doomed) {
      const migration = byId.get(record.id)!;
      if (!migration.downSql) {
        throw new MigrationError(
          `Migration "${migration.filename}" has no down section; add a "-- migrate:down" block to revert it.`,
        );
      }

      if (options.dryRun) {
        results.push({
          id: migration.id,
          filename: migration.filename,
          direction: "down",
          status: "planned",
          durationMs: 0,
        });
        continue;
      }

      const startedAt = Date.now();
      try {
        this.revertOne(migration);
        results.push({
          id: migration.id,
          filename: migration.filename,
          direction: "down",
          status: "reverted",
          durationMs: Date.now() - startedAt,
        });
      } catch (error) {
        const failure: MigrationResult = {
          id: migration.id,
          filename: migration.filename,
          direction: "down",
          status: "skipped",
          durationMs: Date.now() - startedAt,
          error: error instanceof Error ? error.message : String(error),
        };
        results.push(failure);
        this.logger.error(
          { migration: migration.filename, err: error },
          "down migration failed and was rolled back",
        );
        if (options.throwOnError === false) {
          return results;
        }
        throw new MigrationFailedError(failure, results);
      }
    }
    return results;
  }

  /** Run the migration SQL and record it as a single atomic unit. */
  private applyOne(migration: Migration): void {
    const run = this.db.transaction(() => {
      this.db.exec(migration.upSql);
      this.db
        .prepare(
          `INSERT INTO schema_migrations (id, filename, name, checksum, sql, down_sql, applied_at)
           VALUES (@id, @filename, @name, @checksum, @sql, @downSql, @appliedAt)`,
        )
        .run({
          id: migration.id,
          filename: migration.filename,
          name: migration.name,
          checksum: migration.checksum,
          sql: migration.upSql,
          downSql: migration.downSql,
          appliedAt: new Date().toISOString(),
        });
    });
    run();
  }

  /** Run the down SQL and drop its bookkeeping row as a single atomic unit. */
  private revertOne(migration: Migration): void {
    const run = this.db.transaction(() => {
      this.db.exec(migration.downSql as string);
      this.db.prepare("DELETE FROM schema_migrations WHERE id = ?").run(migration.id);
    });
    run();
  }

  /**
   * Fail fast when a migration that is already applied has been edited.
   * Editing applied files is the usual cause of drift between environments.
   */
  private assertNoDrift(): void {
    const byId = new Map(this.migrations.map((migration) => [migration.id, migration]));
    for (const record of this.listApplied()) {
      const migration = byId.get(record.id);
      if (!migration) {
        throw new MigrationError(
          `Migration ${record.id} (${record.filename}) is recorded as applied but its file is missing. ` +
            "Restore the file — applied migrations must not be deleted.",
        );
      }
      if (migration.checksum !== record.checksum) {
        throw new MigrationChecksumError(migration, record.checksum, migration.checksum);
      }
    }
  }
}
