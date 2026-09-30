import { z } from "zod";

// eslint-disable-next-line @typescript-eslint/no-var-requires
const pkg = require("../../package.json");

/**
 * Reject known placeholder values for secret-bearing configuration keys.
 * This prevents accidental deployment with placeholder values from .env.example.
 */
function rejectPlaceholder(value: string, ctx: z.RefinementCtx): void {
  const lowerValue = value.toLowerCase();
  const placeholderPatterns = [
    /^your_.*_here$/,
    /^test-.*$/,
    /^change-in-production$/,
    /^dev-.*$/,
    /^default-.*$/,
  ];

  for (const pattern of placeholderPatterns) {
    if (pattern.test(lowerValue)) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: `Value "${value}" appears to be a placeholder. Please provide a real secret value.`,
      });
      return;
    }
  }
}

const envSchema = z.object({
  PORT: z.coerce.number().int().positive().default(3001),
  NODE_ENV: z.enum(["development", "test", "production"]).default("development"),
  LOG_LEVEL: z
    .enum(["fatal", "error", "warn", "info", "debug", "trace", "silent"])
    .default("info"),

  STELLAR_NETWORK: z.enum(["testnet", "mainnet", "local", "futurenet"]).default("testnet"),
  STELLAR_HORIZON_URL: z.string().url().default("https://horizon-testnet.stellar.org"),
  STELLAR_HORIZON: z.string().url().optional(),
  STELLAR_PUBLIC_KEY: z.string().optional(),
  SKIP_STELLAR_ACCOUNT_VERIFY: z
    .enum(["true", "false"])
    .transform((v) => v === "true")
    .default("false"),
  SOROBAN_RPC_URL: z.string().url().default("https://soroban-testnet.stellar.org"),
  REGISTRY_CONTRACT_ID: z.string().optional(),
  VENICE_API_KEY: z.string().min(1, "VENICE_API_KEY is required"),
  // Filesystem path to the SQLite database that holds the ai-net schema.
  // Applied by `npm run db:migrate`, which resolves it via
  // `resolveDatabasePath()` in src/db/index.ts.
  VENICE_BASE_URL: z.string().url().default("https://api.venice.ai/api/v1"),
  DATABASE_URL: z.string().min(1, "DATABASE_URL is required").default("./data/ai-net.db"),
  // Overrides the location of the versioned migration files. Only needed when
  // migrations are kept outside the repository's `src/db/migrations` folder.
  DB_MIGRATIONS_DIR: z.string().optional(),
  STELLAR_COORDINATOR_SECRET: z.string().optional().superRefine(rejectPlaceholder),
  STELLAR_TEST_SECRET: z.string().optional().superRefine(rejectPlaceholder),
  ALLOWED_ORIGINS: z.string().default("http://localhost:3000"),
  NPM_PACKAGE_VERSION: z.string().default(pkg.version ?? "0.1.0"),
  GRACEFUL_SHUTDOWN_TIMEOUT: z.coerce.number().int().positive().default(30),
  HEALTH_PROBE_TIMEOUT_MS: z.coerce.number().int().positive().default(5_000),

  CACHE_DRIVER: z.enum(["lru", "redis"]).default("lru"),
  REDIS_URL: z.string().default("redis://localhost:6379"),
  CACHE_LRU_MAX_SIZE: z.coerce.number().int().positive().default(500),
  CACHE_TTL_AGENTS: z.coerce.number().int().nonnegative().default(60),
  CACHE_TTL_STATS: z.coerce.number().int().nonnegative().default(30),
  CACHE_TTL_HEALTH: z.coerce.number().int().nonnegative().default(10),
  /** Deployment-scoped key prefix for registry cache entries (Issue #427). */
  REGISTRY_CACHE_KEY_PREFIX: z.string().default("registry"),

  MAX_PROMPT_LENGTH: z.coerce.number().int().positive().default(10_000),
  RATE_LIMIT_WINDOW_MS: z.coerce.number().int().positive().default(60_000),
  RATE_LIMIT_MAX_REQUESTS: z.coerce.number().int().positive().default(20),
  REGISTER_RATE_LIMIT_MAX_REQUESTS: z.coerce.number().int().positive().default(10),
  DAILY_TASK_LIMIT_PER_WALLET: z.coerce.number().int().min(0).default(100),

  /** Token budget management and per-task cost tracking (Issue #390). */
  /** Total tokens (input + output) a single task may consume before it halts. */
  TASK_TOKEN_BUDGET: z.coerce.number().int().positive().default(200_000),
  /** Ceiling on one LLM call's max_tokens. */
  LLM_MAX_TOKENS_PER_CALL: z.coerce.number().int().positive().default(8_192),
  /** Ceiling on one LLM call's input prompt; longer prompts are trimmed. */
  LLM_MAX_PROMPT_TOKENS: z.coerce.number().int().positive().default(16_000),
  /** `MODEL=inputUsd:outputUsd,MODEL=...` overrides for the pricing table. */
  VENICE_PRICING: z.string().optional(),
  /** How often in-flight task costs are flushed to the database (ms). */
  COST_FLUSH_INTERVAL_MS: z.coerce.number().int().positive().default(60_000),

  HEARTBEAT_INTERVAL_MS: z.coerce.number().int().positive().default(300_000),
  HEARTBEAT_STALE_THRESHOLD_MINUTES: z.coerce.number().int().positive().default(5),
  AGENT_OFFLINE_DELETE_HOURS: z.coerce.number().int().positive().default(24),

  /** Agent heartbeat watchdog: grace period before eviction (Issue #379). */
  AGENT_WATCHDOG_INTERVAL_MS: z.coerce.number().int().positive().default(60_000),
  AGENT_WATCHDOG_GRACE_MINUTES: z.coerce.number().int().positive().default(10),

  RECONCILIATION_WEBHOOK_URL: z.string().url().optional(),
  RECONCILIATION_INTERVAL_MS: z.coerce.number().int().positive().default(86_400_000),

  COMPRESSION_THRESHOLD: z.coerce.number().int().min(0).default(1024),
  COMPRESSION_LEVEL: z.coerce.number().int().min(1).max(9).default(6),
  COMPRESSION_ENABLE_BROTLI: z.enum(["true", "false"]).transform((v) => v === "true").default("true"),

  API_LATEST_VERSION: z.string().default("2.0"),
  API_SUPPORTED_VERSIONS: z.string().default("1.0,1.1,2.0"),
  API_DEFAULT_VERSION: z.string().default("1.0"),
  API_V1_SUNSET_DATE: z.string().optional(),

  ADMIN_API_KEY: z.string().min(1).optional(),
  API_KEYS: z.string().optional(),

  DB_POOL_MIN: z.coerce.number().int().positive().default(2),
  DB_POOL_MAX: z.coerce.number().int().positive().default(10),
  DB_POOL_ACQUIRE_TIMEOUT_MS: z.coerce.number().int().positive().default(5_000),
  DB_POOL_HEALTH_CHECK: z
    .enum(["true", "false"])
    .transform((v) => v === "true")
    .default("true"),
  DB_BACKUP_DIR: z.string().default("./data/backups"),
  DB_BACKUP_RETENTION_COUNT: z.coerce.number().int().positive().default(5),
  DB_MAINTENANCE_INTERVAL_MS: z.coerce.number().int().positive().default(3_600_000),
  DB_MAINTENANCE_VACUUM_THRESHOLD: z.coerce.number().int().nonnegative().default(100),
  ERROR_REGISTRY_MAINTENANCE_INTERVAL_MS: z.coerce.number().int().positive().default(3_600_000),
  ERROR_REGISTRY_CAP_PER_AGENT: z.coerce.number().int().positive().default(100),

  // ── Event store retention & compaction (Issue #383) ─────────────────────────
  /**
   * On-disk path for the append-only event store.  A file path is required for
   * the retention job to be meaningful — with `:memory:` the whole event log is
   * discarded on restart, so there is nothing to archive or compact.
   */
  EVENT_STORE_PATH: z.string().default("./data/events.db"),
  /**
   * Retention window in days.  Events belonging to a *finished* task whose most
   * recent event is older than this are archived and then purged from the live
   * `task_events` table.  Days (not row counts) because the boundary is task
   * age, not table pressure.
   */
  EVENT_RETENTION_DAYS: z.coerce.number().int().positive().default(30),
  /** How often the compaction pass runs, in milliseconds. */
  EVENT_COMPACTION_INTERVAL_MS: z.coerce.number().int().positive().default(3_600_000),
  /**
   * Maximum number of tasks compacted per pass.  Bounds the work (and therefore
   * the writer-lock hold time) of a single tick so the live event path is not
   * starved by a large backlog.
   */
  EVENT_COMPACTION_BATCH_TASKS: z.coerce.number().int().positive().default(50),
  /** Master switch for the retention job.  Also disabled when NODE_ENV=test. */
  EVENT_COMPACTION_ENABLED: z
    .enum(["true", "false"])
    .transform((v) => v === "true")
    .default("true"),

  // ── Idempotency store (Issue #657) ───────────────────────────────────────────
  /** How long idempotency keys are retained before they can be replayed. Default: 24 h. */
  IDEMPOTENCY_TTL_MS: z.coerce.number().int().positive().default(86_400_000),
  /** How often the background cleanup sweep runs to delete expired keys. Default: 5 min. */
  IDEMPOTENCY_CLEANUP_MS: z.coerce.number().int().positive().default(300_000),

  WS_MAX_CONNECTIONS_PER_CLIENT: z.coerce.number().int().positive().default(5),
  WS_MAX_MESSAGES_PER_MINUTE: z.coerce.number().int().positive().default(100),
  WS_INACTIVITY_TIMEOUT_MS: z.coerce.number().int().positive().default(1_800_000),
  WS_HEARTBEAT_INTERVAL_MS: z.coerce.number().int().positive().default(30_000),
  WS_PONG_TIMEOUT_MS: z.coerce.number().int().positive().default(10_000),

  METRICS_CACHE_TTL_MS: z.coerce.number().int().positive().default(5_000),
  METRICS_WINDOW_MS: z.coerce.number().int().positive().default(60_000),
  METRICS_MAX_SAMPLES: z.coerce.number().int().positive().default(1_000),

  // ── Authentication & Session Security ───────────────────────────────────────
  /** JWT secret key used to sign and verify access tokens. */
  AUTH_JWT_SECRET: z
    .string()
    .min(32, "AUTH_JWT_SECRET must be at least 32 characters")
    .superRefine(rejectPlaceholder),
  /** Access token validity in seconds. Default: 900 (15 min). */
  AUTH_ACCESS_TOKEN_TTL_SECONDS: z.coerce.number().int().positive().default(900),
  /** Refresh token sliding expiry validity in seconds. Default: 604 800 (7 days). */
  AUTH_REFRESH_TOKEN_TTL_SECONDS: z.coerce.number().int().positive().default(604_800),
  /** Max absolute session lifetime in seconds. Default: 2 592 000 (30 days). */
  AUTH_SESSION_MAX_TTL_SECONDS: z.coerce.number().int().positive().default(2_592_000),

  // ── Quality Scorer Configuration ─────────────────────────────────────────────
  /** Weight for completeness score in quality calculation. Default: 0.4 */
  QUALITY_WEIGHT_COMPLETENESS: z.coerce.number().min(0).max(1).default(0.4),
  /** Weight for relevance score in quality calculation. Default: 0.3 */
  QUALITY_WEIGHT_RELEVANCE: z.coerce.number().min(0).max(1).default(0.3),
  /** Weight for format score in quality calculation. Default: 0.3 */
  QUALITY_WEIGHT_FORMAT: z.coerce.number().min(0).max(1).default(0.3),
  /** Threshold for requiring manual review. Default: 60 */
  QUALITY_REVIEW_THRESHOLD: z.coerce.number().int().min(0).max(100).default(60),
  /** Enable percentile-based quality scoring. Default: false */
  QUALITY_PERCENTILE_ENABLED: z.enum(["true", "false"]).transform((v) => v === "true").default("false"),
  /** Minimum samples required for percentile calculation. Default: 10 */
  QUALITY_PERCENTILE_MIN_SAMPLES: z.coerce.number().int().positive().default(10),

  // ── Admin Control Configuration ─────────────────────────────────────────────
  /** Enable read-only mode for the API. Default: false */
  AI_NET_READ_ONLY: z.enum(["true", "false"]).transform((v) => v === "true").default("false"),
  /** Reason for read-only mode. Optional. */
  AI_NET_READ_ONLY_REASON: z.string().optional(),
  /** Path to admin audit database. Default: ./data/admin-audit.db */
  ADMIN_AUDIT_DB_PATH: z.string().default("./data/admin-audit.db"),
  /** Directory for admin backups. Default: ./data/backups/admin */
  ADMIN_BACKUP_DIR: z.string().default("./data/backups/admin"),

  // ── Agent Watchdog Configuration ──────────────────────────────────────────────
  /** Agent heartbeat watchdog: grace period before eviction (Issue #379). */
  AGENT_WATCHDOG_INTERVAL_MS: z.coerce.number().int().positive().default(60_000),
  AGENT_WATCHDOG_GRACE_MINUTES: z.coerce.number().int().positive().default(10),

  // ── Idempotency Store Configuration (Issue #657) ────────────────────────────
  /** How long idempotency keys are retained before they can be replayed. Default: 24 h. */
  IDEMPOTENCY_TTL_MS: z.coerce.number().int().positive().default(86_400_000),
  /** How often the background cleanup sweep runs to delete expired keys. Default: 5 min. */
  IDEMPOTENCY_CLEANUP_MS: z.coerce.number().int().positive().default(300_000),

  // ── Rate Limit Configuration ─────────────────────────────────────────────────
  /** Per-route-group limits (token-bucket, per IP, rolling window) */
  RATE_LIMIT_PUBLIC_WINDOW_MS: z.coerce.number().int().positive().default(60_000),
  RATE_LIMIT_PUBLIC_MAX_REQUESTS: z.coerce.number().int().positive().default(120),
  RATE_LIMIT_AUTHED_WINDOW_MS: z.coerce.number().int().positive().default(60_000),
  RATE_LIMIT_AUTHED_MAX_REQUESTS: z.coerce.number().int().positive().default(30),
  RATE_LIMIT_ADMIN_WINDOW_MS: z.coerce.number().int().positive().default(60_000),
  RATE_LIMIT_ADMIN_MAX_REQUESTS: z.coerce.number().int().positive().default(20),

  // ── Venice Cache Configuration ───────────────────────────────────────────────
  /** Logical model version used in Venice response cache keys. */
  VENICE_MODEL_VERSION: z.string().default("v1"),
  /** Default Venice response cache TTL in milliseconds. */
  VENICE_CACHE_TTL_MS: z.coerce.number().int().positive().default(86_400_000),
  /** Shorter Venice cache TTL for coding-agent responses in milliseconds. */
  VENICE_CACHE_CODING_TTL_MS: z.coerce.number().int().positive().default(3_600_000),
  /** Similarity threshold for semantic Venice cache reuse. */
  VENICE_CACHE_SIMILARITY_THRESHOLD: z.coerce.number().min(0).max(1).default(0.8),
});

export type RawConfig = z.infer<typeof envSchema>;
export type Config = RawConfig & {
  STELLAR_NETWORK_PASSPHRASE: string;
};

export class ConfigValidationError extends Error {
  constructor(readonly issues: z.ZodIssue[]) {
    super(
      `[config] Invalid environment variables:\n${issues
        .map((issue) => `  ${issue.path.join(".") || "ENV"}: ${issue.message}`)
        .join("\n")}`,
    );
    this.name = "ConfigValidationError";
  }
}

let cachedConfig: Config | null = null;

function emptyToUndefined(value: unknown): unknown {
  return typeof value === "string" && value.trim() === "" ? undefined : value;
}

function withRuntimeDefaults(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const nodeEnv = env.NODE_ENV ?? "development";
  const testDefaults =
    nodeEnv === "test"
      ? {
          DATABASE_URL: ":memory:",
          // Keep the event store off the filesystem under test, mirroring
          // DATABASE_URL. Tests that need real persistence pass an explicit
          // path to createEventStore() instead.
          EVENT_STORE_PATH: ":memory:",
          VENICE_API_KEY: "test-venice-key",
          LOG_LEVEL: "silent",
          AUTH_JWT_SECRET: "test-jwt-secret-for-development-only",
        }
      : {};

  const devDefaults =
    nodeEnv === "development"
      ? {
          AUTH_JWT_SECRET: env.AUTH_JWT_SECRET ?? "dev-jwt-secret-change-in-production",
        }
      : {};

  return {
    ...testDefaults,
    ...devDefaults,
    ...env,
    STELLAR_HORIZON_URL: env.STELLAR_HORIZON_URL ?? env.STELLAR_HORIZON,
  };
}

function networkPassphrase(network: RawConfig["STELLAR_NETWORK"]): string {
  switch (network) {
    case "mainnet":
      return "Public Global Stellar Network ; September 2015";
    case "local":
      return "Standalone Network ; February 2017";
    case "futurenet":
      return "Test SDF Future Network ; October 2022";
    case "testnet":
    default:
      return "Test SDF Network ; September 2015";
  }
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const result = envSchema.safeParse(withRuntimeDefaults(env));

  if (!result.success) {
    throw new ConfigValidationError(result.error.issues);
  }

  const nodeEnv = result.data.NODE_ENV;

  // Fail closed in production: AUTH_JWT_SECRET must be explicitly set
  if (nodeEnv === "production") {
    const providedSecret = env.AUTH_JWT_SECRET;
    if (!providedSecret) {
      throw new ConfigValidationError([
        {
          code: z.ZodIssueCode.custom,
          path: ["AUTH_JWT_SECRET"],
          message: "AUTH_JWT_SECRET is required in production",
        },
      ]);
    }
  }

  // Warn in development if using default secret
  if (nodeEnv === "development") {
    const secret = result.data.AUTH_JWT_SECRET;
    if (secret === "dev-jwt-secret-change-in-production") {
      console.warn(
        "[config] WARNING: Using default AUTH_JWT_SECRET in development. " +
          "Set AUTH_JWT_SECRET to a secure random value in production."
      );
    }
  }

  cachedConfig = {
    ...result.data,
    STELLAR_NETWORK_PASSPHRASE: networkPassphrase(result.data.STELLAR_NETWORK),
  };
  return cachedConfig;
}

export function getConfig(): Config {
  return cachedConfig ?? loadConfig();
}

export function resetConfigForTests(): void {
  cachedConfig = null;
}

export const config = new Proxy({} as Config, {
  get(_target, property: keyof Config) {
    return getConfig()[property];
  },
});

export function ttlForRoute(group: "agents" | "stats" | "health"): number {
  const cfg = getConfig();
  switch (group) {
    case "agents":
      return cfg.CACHE_TTL_AGENTS;
    case "stats":
      return cfg.CACHE_TTL_STATS;
    case "health":
      return cfg.CACHE_TTL_HEALTH;
  }
}

export function allowedOrigins(): string[] {
  return getConfig()
    .ALLOWED_ORIGINS.split(",")
    .map((origin) => origin.trim())
    .filter(Boolean);
}

function redactConfigValue(key: string, value: unknown): unknown {
  if (/secret|token|api[_-]?key|password|authorization|cookie|private[_-]?key/i.test(key)) {
    return value ? "[REDACTED]" : value;
  }
  if (/address|public[_-]?key|wallet|owner|claimant|destination|source/i.test(key)) {
    return value ? "[REDACTED_ADDRESS]" : value;
  }
  return value;
}

export function redactedConfigSnapshot(cfg: Config = getConfig()): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(cfg).map(([key, value]) => [key, redactConfigValue(key, value)]),
  );
}