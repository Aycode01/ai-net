/**
 * TypeScript SDK wrapper for the Task Store Soroban contract.
 *
 * Provides a typed client interface for the on-chain task metadata store that
 * manages the task lifecycle state machine and optional oracle price stamping.
 *
 * Key operations:
 *  - `storeTask`        – create a new task, optionally resolving a price via oracle
 *  - `updateTaskStatus` – advance the task through its lifecycle states
 *  - `finalizeTask`     – mark a task as completed or failed
 *  - `getTask`          – fetch task metadata (read-only)
 *  - `getTaskStatus`    – fetch just the status (read-only)
 *  - `setOracleManager` – configure the oracle manager for price resolution
 *
 * Mirrors the Rust contract interface in contracts/task_store/src/lib.rs.
 *
 * Issue #490 — TypeScript SDK wrappers for all Soroban contracts
 */

// ---------------------------------------------------------------------------
// Shared transaction wrapper (mirrors coordinator.ts pattern)
// ---------------------------------------------------------------------------

/** Thin wrapper around an assembled Soroban transaction. */
export interface AssembledTransaction<T> {
  signAndSend(): Promise<T>;
  simulate(): Promise<T>;
}

// ---------------------------------------------------------------------------
// Domain types (mirrors task_store/src/types.rs)
// ---------------------------------------------------------------------------

/** Task lifecycle states */
export enum TaskStatus {
  Pending = 0,
  Running = 1,
  Completed = 2,
  Failed = 3,
}

export interface TaskMetadata {
  taskId: Uint8Array;
  submitter: string;
  promptHash: Uint8Array;
  assignedAgents: string[];
  /** DEFLATE-compressed DAG payload */
  compressedDag: Uint8Array;
  status: TaskStatus;
  createdAt: bigint;
  expiresAt: bigint;
  /** Price in stroops resolved via oracle at task creation time, if configured */
  quotedPriceStroops?: bigint;
}

export interface StoreTaskInput {
  submitter: string;
  taskId: Uint8Array;
  promptHash: Uint8Array;
  assignedAgents: string[];
  compressedDag: Uint8Array;
  /** Time-to-live in days (0 = use contract default) */
  ttlDays?: number;
  /** Price pair for oracle resolution, e.g. "XLM/USDC" */
  pricePair?: string;
}

// ---------------------------------------------------------------------------
// Contract client interface
// ---------------------------------------------------------------------------

export interface TaskStoreContractClient {
  initialize(args: { admin: string }): AssembledTransaction<void>;

  store_task(args: {
    submitter: string;
    task_id: Uint8Array;
    prompt_hash: Uint8Array;
    assigned_agents: string[];
    compressed_dag: Uint8Array;
    ttl_days: number;
    price_pair?: string;
  }): AssembledTransaction<void>;

  get_task(task_id: Uint8Array): AssembledTransaction<TaskMetadata>;

  get_task_status(task_id: Uint8Array): AssembledTransaction<TaskStatus>;

  update_task_status(args: {
    task_id: Uint8Array;
    agent: string;
    new_status: TaskStatus;
  }): AssembledTransaction<void>;

  finalize_task(args: {
    task_id: Uint8Array;
    agent: string;
    success: boolean;
  }): AssembledTransaction<void>;

  set_oracle_manager(args: {
    admin: string;
    oracle_manager: string;
  }): AssembledTransaction<void>;
}

// ---------------------------------------------------------------------------
// High-level SDK wrapper class
// ---------------------------------------------------------------------------

/**
 * TaskStoreSDK — high-level wrapper around the TaskStore Soroban contract.
 *
 * @example
 * ```ts
 * const sdk = new TaskStoreSDK(client);
 *
 * await sdk.storeTask({
 *   submitter: 'GSUB...',
 *   taskId: crypto.getRandomValues(new Uint8Array(32)),
 *   promptHash: sha256('my task prompt'),
 *   assignedAgents: ['GAGENT1...', 'GAGENT2...'],
 *   compressedDag: deflateSync(Buffer.from(JSON.stringify(dag))),
 *   ttlDays: 7,
 *   pricePair: 'XLM/USDC',
 * });
 * ```
 */
export class TaskStoreSDK {
  constructor(private readonly client: TaskStoreContractClient) {}

  /**
   * Initialise the contract with the given admin address.
   */
  async initialize(admin: string): Promise<void> {
    return this.client.initialize({ admin }).signAndSend();
  }

  /**
   * Store task metadata on-chain. If `pricePair` is provided and an oracle
   * manager is configured, the current market price is stamped immutably.
   */
  async storeTask(input: StoreTaskInput): Promise<void> {
    return this.client
      .store_task({
        submitter: input.submitter,
        task_id: input.taskId,
        prompt_hash: input.promptHash,
        assigned_agents: input.assignedAgents,
        compressed_dag: input.compressedDag,
        ttl_days: input.ttlDays ?? 0,
        price_pair: input.pricePair,
      })
      .signAndSend();
  }

  /**
   * Fetch task metadata by task ID (read-only).
   */
  async getTask(taskId: Uint8Array): Promise<TaskMetadata> {
    return this.client.get_task(taskId).simulate();
  }

  /**
   * Fetch only the task status (read-only, cheaper simulation).
   */
  async getTaskStatus(taskId: Uint8Array): Promise<TaskStatus> {
    return this.client.get_task_status(taskId).simulate();
  }

  /**
   * Advance the task status in the lifecycle state machine.
   */
  async updateTaskStatus(args: {
    taskId: Uint8Array;
    agent: string;
    newStatus: TaskStatus;
  }): Promise<void> {
    return this.client
      .update_task_status({
        task_id: args.taskId,
        agent: args.agent,
        new_status: args.newStatus,
      })
      .signAndSend();
  }

  /**
   * Finalise a task as completed or failed. This is the terminal state transition.
   */
  async finalizeTask(args: {
    taskId: Uint8Array;
    agent: string;
    success: boolean;
  }): Promise<void> {
    return this.client
      .finalize_task({
        task_id: args.taskId,
        agent: args.agent,
        success: args.success,
      })
      .signAndSend();
  }

  /**
   * Configure the oracle manager contract address (admin only).
   */
  async setOracleManager(args: {
    admin: string;
    oracleManager: string;
  }): Promise<void> {
    return this.client
      .set_oracle_manager({ admin: args.admin, oracle_manager: args.oracleManager })
      .signAndSend();
  }
}
