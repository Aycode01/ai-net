/**
 * Venice-scoped circuit breaker.
 *
 * Re-exports the generalised `CircuitBreaker` from `../circuitBreaker` with
 * Venice-specific defaults so existing Venice client code continues to work
 * without modification.
 */

export type { CircuitState } from '../circuitBreaker.js';
export { CircuitBreaker } from '../circuitBreaker.js';
export { CircuitOpenError } from '../circuitBreaker.js';
