/**
 * TypeScript SDK wrapper for the Dispute Resolution Soroban contract.
 *
 * Provides a typed client interface for the on-chain dispute resolution system
 * that manages evidence submission, juror voting, and resolution enforcement.
 *
 * Lifecycle:
 *  1. `openDispute`     – a party raises a dispute for a task/booking
 *  2. `submitEvidence`  – parties submit supporting evidence during evidence phase (3 days)
 *  3. `castVote`        – selected jurors vote during the voting phase (2 days)
 *  4. `resolveDispute`  – anyone can trigger resolution after the voting phase
 *  5. `appeal`          – losing party may appeal within the appeal window (2 days)
 *  6. `claimJurorReward`– jurors aligned with the majority claim their reward
 *
 * Mirrors the Rust contract interface in contracts/dispute_resolution/src/lib.rs.
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
// Domain types
// ---------------------------------------------------------------------------

/** Duration constants (mirrors the Rust consts) */
export const EVIDENCE_PHASE_SECS = 259_200; // 3 days
export const VOTING_PHASE_SECS = 172_800;   // 2 days
export const APPEAL_WINDOW_SECS = 172_800;  // 2 days
export const JUROR_COUNT = 5;

export enum DisputePhase {
  Evidence = 'Evidence',
  Voting = 'Voting',
  Resolved = 'Resolved',
  Appealed = 'Appealed',
  Closed = 'Closed',
}

export enum DisputeOutcome {
  Claimant = 'Claimant',
  Respondent = 'Respondent',
  Draw = 'Draw',
  Unresolved = 'Unresolved',
}

export interface Evidence {
  evidenceId: number;
  submitter: string;
  contentHash: Uint8Array;
  submittedAt: bigint;
}

export interface JurorVote {
  juror: string;
  vote: 'Claimant' | 'Respondent';
  votedAt: bigint;
}

export interface DisputeInfo {
  disputeId: string;
  taskId: string;
  claimant: string;
  respondent: string;
  phase: DisputePhase;
  outcome: DisputeOutcome;
  openedAt: bigint;
  evidenceDeadline: bigint;
  votingDeadline: bigint;
  appealDeadline: bigint;
  jurors: string[];
  claimantVotes: number;
  respondentVotes: number;
}

// ---------------------------------------------------------------------------
// Contract client interface
// ---------------------------------------------------------------------------

export interface DisputeResolutionContractClient {
  initialize(args: { admin: string }): AssembledTransaction<void>;

  open_dispute(args: {
    task_id: string;
    claimant: string;
    respondent: string;
  }): AssembledTransaction<string>;

  submit_evidence(args: {
    dispute_id: string;
    submitter: string;
    content_hash: Uint8Array;
  }): AssembledTransaction<number>;

  cast_vote(args: {
    dispute_id: string;
    juror: string;
    vote: 'Claimant' | 'Respondent';
  }): AssembledTransaction<void>;

  resolve_dispute(args: {
    dispute_id: string;
  }): AssembledTransaction<DisputeOutcome>;

  appeal(args: {
    dispute_id: string;
    appellant: string;
  }): AssembledTransaction<void>;

  claim_juror_reward(args: {
    dispute_id: string;
    juror: string;
  }): AssembledTransaction<bigint>;

  get_dispute(dispute_id: string): AssembledTransaction<DisputeInfo>;

  get_evidence(args: {
    dispute_id: string;
    evidence_id: number;
  }): AssembledTransaction<Evidence>;

  get_juror_vote(args: {
    dispute_id: string;
    juror: string;
  }): AssembledTransaction<JurorVote>;
}

// ---------------------------------------------------------------------------
// High-level SDK wrapper class
// ---------------------------------------------------------------------------

/**
 * DisputeResolutionSDK — high-level wrapper around the DisputeResolution Soroban contract.
 *
 * @example
 * ```ts
 * const sdk = new DisputeResolutionSDK(client);
 *
 * const disputeId = await sdk.openDispute({
 *   taskId: 'task-001',
 *   claimant: 'GCLAIM...',
 *   respondent: 'GRESP...',
 * });
 * ```
 */
export class DisputeResolutionSDK {
  constructor(private readonly client: DisputeResolutionContractClient) {}

  /**
   * Initialise the contract with the given admin address.
   */
  async initialize(admin: string): Promise<void> {
    return this.client.initialize({ admin }).signAndSend();
  }

  /**
   * Open a new dispute for a task. Returns the generated dispute ID.
   */
  async openDispute(args: {
    taskId: string;
    claimant: string;
    respondent: string;
  }): Promise<string> {
    return this.client
      .open_dispute({
        task_id: args.taskId,
        claimant: args.claimant,
        respondent: args.respondent,
      })
      .signAndSend();
  }

  /**
   * Submit evidence during the evidence phase.
   * @returns The assigned evidence ID.
   */
  async submitEvidence(args: {
    disputeId: string;
    submitter: string;
    contentHash: Uint8Array;
  }): Promise<number> {
    return this.client
      .submit_evidence({
        dispute_id: args.disputeId,
        submitter: args.submitter,
        content_hash: args.contentHash,
      })
      .signAndSend();
  }

  /**
   * Cast a juror vote during the voting phase.
   */
  async castVote(args: {
    disputeId: string;
    juror: string;
    vote: 'Claimant' | 'Respondent';
  }): Promise<void> {
    return this.client
      .cast_vote({
        dispute_id: args.disputeId,
        juror: args.juror,
        vote: args.vote,
      })
      .signAndSend();
  }

  /**
   * Trigger resolution after the voting phase. Returns the outcome.
   */
  async resolveDispute(disputeId: string): Promise<DisputeOutcome> {
    return this.client.resolve_dispute({ dispute_id: disputeId }).signAndSend();
  }

  /**
   * File an appeal within the appeal window.
   */
  async appeal(args: { disputeId: string; appellant: string }): Promise<void> {
    return this.client
      .appeal({ dispute_id: args.disputeId, appellant: args.appellant })
      .signAndSend();
  }

  /**
   * Claim juror reward for majority-aligned vote.
   * @returns The XLM reward amount in stroops.
   */
  async claimJurorReward(args: {
    disputeId: string;
    juror: string;
  }): Promise<bigint> {
    return this.client
      .claim_juror_reward({ dispute_id: args.disputeId, juror: args.juror })
      .signAndSend();
  }

  /**
   * Fetch dispute metadata (read-only).
   */
  async getDispute(disputeId: string): Promise<DisputeInfo> {
    return this.client.get_dispute(disputeId).simulate();
  }

  /**
   * Fetch a specific piece of evidence (read-only).
   */
  async getEvidence(args: {
    disputeId: string;
    evidenceId: number;
  }): Promise<Evidence> {
    return this.client
      .get_evidence({ dispute_id: args.disputeId, evidence_id: args.evidenceId })
      .simulate();
  }

  /**
   * Fetch a juror's vote for a dispute (read-only).
   */
  async getJurorVote(args: {
    disputeId: string;
    juror: string;
  }): Promise<JurorVote> {
    return this.client
      .get_juror_vote({ dispute_id: args.disputeId, juror: args.juror })
      .simulate();
  }
}
