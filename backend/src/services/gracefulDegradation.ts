/**
 * Graceful degradation helpers.
 *
 * When a circuit breaker is OPEN the service cannot reach the upstream
 * dependency. These utilities provide cached or static fallback responses
 * so the system continues to function in a degraded — but not broken — state.
 */

import { createLogger } from '../utils/logger.js';
import { CircuitOpenError } from './circuitBreaker.js';

const log = createLogger({ module: 'GracefulDegradation' });

export interface FallbackOptions<T> {
  /** Human-readable service name for log messages. */
  serviceName: string;
  /** The primary operation to attempt. */
  operation: () => Promise<T>;
  /** Fallback to return when the circuit is open or the call fails. */
  fallback: () => T | Promise<T>;
  /**
   * When true, the fallback is also used for non-circuit-breaker errors.
   * Defaults to false — non-circuit errors are rethrown.
   */
  fallbackOnAnyError?: boolean;
}

/**
 * Execute `operation`; if a `CircuitOpenError` is thrown (or any error when
 * `fallbackOnAnyError` is true) return the result of `fallback` instead.
 *
 * Logs a warning whenever the fallback path is taken.
 */
export async function withFallback<T>(options: FallbackOptions<T>): Promise<T> {
  const { serviceName, operation, fallback, fallbackOnAnyError = false } = options;
  try {
    return await operation();
  } catch (err) {
    if (err instanceof CircuitOpenError || fallbackOnAnyError) {
      log.warn(
        { service: serviceName, circuitOpen: err instanceof CircuitOpenError },
        'Using cached/fallback response due to circuit breaker or service error'
      );
      return fallback();
    }
    throw err;
  }
}

/**
 * Simple in-memory TTL cache — used by graceful degradation to serve stale
 * data when upstream services are unavailable.
 */
export class ResponseCache<T> {
  private cache = new Map<string, { value: T; expiresAt: number }>();
  private readonly ttlMs: number;

  constructor(ttlMs = 60_000) {
    this.ttlMs = ttlMs;
  }

  set(key: string, value: T): void {
    this.cache.set(key, { value, expiresAt: Date.now() + this.ttlMs });
  }

  get(key: string): T | undefined {
    const entry = this.cache.get(key);
    if (!entry) return undefined;
    if (Date.now() > entry.expiresAt) {
      this.cache.delete(key);
      return undefined;
    }
    return entry.value;
  }

  has(key: string): boolean {
    return this.get(key) !== undefined;
  }

  clear(): void {
    this.cache.clear();
  }
}
