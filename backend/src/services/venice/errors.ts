/**
 * Errors raised by the Venice AI integration.
 *
 * `CircuitOpenError` is deliberately distinct from an upstream error: when the
 * circuit is open the call is shed *before* any HTTP request is made, so callers
 * must be able to tell "the upstream is unavailable" (retryable) apart from
 * "we are protecting the upstream" (shed, retry later).
 */

/** Thrown when a call is rejected by the circuit breaker (no HTTP is performed). */
export class CircuitOpenError extends Error {
  readonly state: 'OPEN' | 'HALF_OPEN';

  constructor(
    message = 'Circuit breaker is OPEN — Venice requests are blocked',
    state: 'OPEN' | 'HALF_OPEN' = 'OPEN',
  ) {
    super(message);
    this.name = 'CircuitOpenError';
    this.state = state;
  }
}

/** Thrown when a request asks for more tokens than the hard cap allows. */
export class TokenBudgetExceededError extends Error {
  constructor(requested: number, cap: number) {
    super(`Token budget exceeded: requested ${requested}, hard cap is ${cap}`);
    this.name = 'TokenBudgetExceededError';
  }
}
