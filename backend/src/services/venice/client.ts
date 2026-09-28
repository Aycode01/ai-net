import { randomUUID } from 'node:crypto';
import { createLogger } from '../../utils/logger.js';
import { CircuitBreaker } from './circuitBreaker.js';
import type { CircuitMetrics, CircuitState, CircuitTransition } from './circuitBreaker.js';
import { CircuitOpenError, TokenBudgetExceededError } from './errors.js';
import { VeniceResponseCache, buildCacheKey } from './cache.js';
import { RequestDeduplicator } from './dedup.js';
import { getConfig } from '../../config/index.js';
import type {
  AgentType,
  CompleteOptions,
  VeniceChatOptions,
  VeniceClientConfig,
  VeniceClientLike,
  VeniceMessage,
  VeniceProviderConfig,
} from './types.js';

interface CacheEnvConfig {
  VENICE_MODEL_VERSION: string;
  VENICE_CACHE_TTL_MS: number;
  VENICE_CACHE_CODING_TTL_MS: number;
  VENICE_CACHE_SIMILARITY_THRESHOLD: number;
  VENICE_REQUEST_TIMEOUT_MS: number;
  VENICE_PROVIDER_MAX_RETRIES: number;
}

/** Circuit-breaker tuning resolved from the environment (issue #495). */
interface CircuitEnvConfig {
  VENICE_CIRCUIT_FAILURE_THRESHOLD: number;
  VENICE_CIRCUIT_COOLDOWN_MS: number;
  VENICE_CIRCUIT_PROBE_COUNT: number;
}

const CONFIG_FALLBACK: CacheEnvConfig = {
  VENICE_MODEL_VERSION: 'v1',
  VENICE_CACHE_TTL_MS: 24 * 60 * 60 * 1000,
  VENICE_CACHE_CODING_TTL_MS: 60 * 60 * 1000,
  VENICE_CACHE_SIMILARITY_THRESHOLD: 0.8,
  VENICE_REQUEST_TIMEOUT_MS: 10_000,
  VENICE_PROVIDER_MAX_RETRIES: 3,
};

const CIRCUIT_CONFIG_FALLBACK: CircuitEnvConfig = {
  VENICE_CIRCUIT_FAILURE_THRESHOLD: 3,
  VENICE_CIRCUIT_COOLDOWN_MS: 60_000,
  VENICE_CIRCUIT_PROBE_COUNT: 1,
};

/**
 * Mirror the breaker's state onto the Prometheus gauge.
 *
 * Loaded lazily so the Venice module keeps no import-time dependency on the
 * metrics service (and therefore on its config/DB wiring); metrics are
 * best-effort telemetry and must never break a Venice call.
 */
function setVeniceCircuitBreakerState(code: number): void {
  try {
    // eslint-disable-next-line @typescript-eslint/no-var-requires, @typescript-eslint/no-require-imports
    const { metricsService } = require('../../services/metrics') as typeof import('../../services/metrics');
    metricsService.setVeniceCircuitBreakerState(code);
  } catch {
    // Metrics subsystem unavailable (e.g. a stripped test harness) — ignore.
  }
}

const log = createLogger({ module: 'VeniceClient' });

const MODEL_MAP: Record<AgentType, string> = {
  research: 'venice-xl',
  risk: 'venice-xl',
  coding: 'venice-code',
  design: 'venice-xl',
  report: 'venice-xl',
};

const DEFAULT_MAX_TOKENS = 2048;
const HARD_TOKEN_CAP = 8192;
const RETRY_DELAYS_MS = [200, 400, 800, 1600];
const RETRYABLE_STATUS_CODES = new Set([429, 503, 500, 502, 504]);
const NON_RETRYABLE_STATUS_CODES = new Set([400, 401, 422]);
const DEFAULT_CHAT_MODEL = 'llama-3.3-70b';

export class VeniceClient implements VeniceClientLike {
  private readonly providers: VeniceProviderConfig[];
  private readonly breaker: CircuitBreaker;
  private readonly cache: VeniceResponseCache;
  private readonly deduplicator: RequestDeduplicator;
  private readonly modelVersion: string;
  private readonly timeoutMs: number;
  private readonly maxRetries: number;
  private readonly enableCacheFallback: boolean;
  /** Unsubscribe handles for the breaker's transition listener. */
  private readonly circuitUnsubscribers: Array<() => void> = [];

  // Backward compat: expose primary for existing callers
  private get apiKey(): string {
    return this.providers[0]?.apiKey ?? '';
  }
  private get baseUrl(): string {
    return this.providers[0]?.baseUrl ?? 'https://api.venice.ai/api/v1';
  }

  constructor(config: VeniceClientConfig) {
    this.breaker = config.circuitBreaker ?? this.createBreaker(config);

    const env = this.resolveConfig() as any;
    this.modelVersion = config.modelVersion ?? env.VENICE_MODEL_VERSION ?? CONFIG_FALLBACK.VENICE_MODEL_VERSION;
    this.timeoutMs = config.timeoutMs ?? env.VENICE_REQUEST_TIMEOUT_MS ?? CONFIG_FALLBACK.VENICE_REQUEST_TIMEOUT_MS;
    this.maxRetries = config.maxRetries ?? env.VENICE_PROVIDER_MAX_RETRIES ?? CONFIG_FALLBACK.VENICE_PROVIDER_MAX_RETRIES;
    this.enableCacheFallback = config.enableCacheFallback ?? true;

    // Surface every state transition (`opened` / `closed` / `half_opened`) both
    // to the caller's listener and to the process-wide metrics gauge.
    this.circuitUnsubscribers.push(
      this.breaker.on('*', (transition) => this.onCircuitTransition(transition)),
    );
    if (config.onCircuitStateChange) {
      this.circuitUnsubscribers.push(
        this.breaker.on('*', (transition) => config.onCircuitStateChange?.(transition)),
      );
    }

    // Build ordered provider chain: explicit providers wins, otherwise build from config + env fallbacks
    if (config.providers && config.providers.length > 0) {
      this.providers = config.providers.map((p) => ({
        apiKey: p.apiKey,
        baseUrl: p.baseUrl ?? this.resolveBaseUrl(),
        name: p.name,
      }));
    } else {
      this.providers = this.buildProvidersFromEnv(config);
    }

    const cacheConfig = config.cacheConfig ?? {};
    this.cache =
      config.cache ??
      new VeniceResponseCache({
        defaultTtlMs: cacheConfig.defaultTtlMs ?? env.VENICE_CACHE_TTL_MS ?? CONFIG_FALLBACK.VENICE_CACHE_TTL_MS,
        codingTtlMs: cacheConfig.codingTtlMs ?? env.VENICE_CACHE_CODING_TTL_MS ?? CONFIG_FALLBACK.VENICE_CACHE_CODING_TTL_MS,
        similarityThreshold:
          cacheConfig.similarityThreshold ?? env.VENICE_CACHE_SIMILARITY_THRESHOLD ?? CONFIG_FALLBACK.VENICE_CACHE_SIMILARITY_THRESHOLD,
      });
    this.deduplicator = config.deduplicator ?? new RequestDeduplicator();
  }

  /**
   * Build a breaker with the same three-state semantics as the injected one,
   * tunable through `VENICE_CIRCUIT_*` env vars (issue #495).
   */
  private createBreaker(config: VeniceClientConfig): CircuitBreaker {
    const env = this.resolveCircuitConfig();
    return new CircuitBreaker({
      failureThreshold:
        config.failureThreshold ??
        env.VENICE_CIRCUIT_FAILURE_THRESHOLD ??
        CIRCUIT_CONFIG_FALLBACK.VENICE_CIRCUIT_FAILURE_THRESHOLD,
      cooldownMs:
        config.cooldownMs ??
        env.VENICE_CIRCUIT_COOLDOWN_MS ??
        CIRCUIT_CONFIG_FALLBACK.VENICE_CIRCUIT_COOLDOWN_MS,
      probeCount:
        config.probeCount ??
        env.VENICE_CIRCUIT_PROBE_COUNT ??
        CIRCUIT_CONFIG_FALLBACK.VENICE_CIRCUIT_PROBE_COUNT,
    });
  }

  /** Fan a breaker transition out to the metrics gauge and the structured log. */
  private onCircuitTransition(transition: CircuitTransition): void {
    // 0 = closed, 1 = open, 2 = half-open (see MetricsService).
    const code = transition.to === 'CLOSED' ? 0 : transition.to === 'OPEN' ? 1 : 2;
    setVeniceCircuitBreakerState(code);
    log.info(
      {
        event: transition.event,
        from: transition.from,
        to: transition.to,
        reason: transition.reason,
        failures: transition.failures,
      },
      'venice circuit transition',
    );
  }


  /** Detach the breaker's transition listeners. */
  dispose(): void {
    for (const unsubscribe of this.circuitUnsubscribers) {
      unsubscribe();
    }
    this.circuitUnsubscribers.length = 0;
  }

  private buildProvidersFromEnv(config: VeniceClientConfig): VeniceProviderConfig[] {
    const primary: VeniceProviderConfig = {
      apiKey: config.apiKey,
      baseUrl: config.baseUrl ?? this.resolveBaseUrl(),
      name: 'primary',
    };
    const providers: VeniceProviderConfig[] = [primary];

    // Try to read fallback env vars via getConfig (if available)
    try {
      const cfg: any = getConfig();
      const fallbackKeys: string = cfg.VENICE_FALLBACK_API_KEYS ?? '';
      const fallbackUrls: string = cfg.VENICE_FALLBACK_BASE_URLS ?? '';
      if (fallbackKeys) {
        const keys = fallbackKeys
          .split(',')
          .map((k: string) => k.trim())
          .filter(Boolean);
        const urls = fallbackUrls
          ? fallbackUrls.split(',').map((u: string) => u.trim()).filter(Boolean)
          : [];
        keys.forEach((key: string, idx: number) => {
          providers.push({
            apiKey: key,
            baseUrl: urls[idx] ?? urls[0] ?? primary.baseUrl ?? 'https://api.venice.ai/api/v1',
            name: `fallback-${idx + 1}`,
          });
        });
      }
    } catch {
      // No config available (e.g. in tests) — just use primary
    }

    return providers;
  }

  private resolveConfig(): CacheEnvConfig {
    try {
      const config = getConfig() as any;
      return {
        VENICE_MODEL_VERSION: config?.VENICE_MODEL_VERSION ?? CONFIG_FALLBACK.VENICE_MODEL_VERSION,
        VENICE_CACHE_TTL_MS: config?.VENICE_CACHE_TTL_MS ?? CONFIG_FALLBACK.VENICE_CACHE_TTL_MS,
        VENICE_CACHE_CODING_TTL_MS: config?.VENICE_CACHE_CODING_TTL_MS ?? CONFIG_FALLBACK.VENICE_CACHE_CODING_TTL_MS,
        VENICE_CACHE_SIMILARITY_THRESHOLD: config?.VENICE_CACHE_SIMILARITY_THRESHOLD ?? CONFIG_FALLBACK.VENICE_CACHE_SIMILARITY_THRESHOLD,
        VENICE_REQUEST_TIMEOUT_MS: config?.VENICE_REQUEST_TIMEOUT_MS ?? CONFIG_FALLBACK.VENICE_REQUEST_TIMEOUT_MS,
        VENICE_PROVIDER_MAX_RETRIES: config?.VENICE_PROVIDER_MAX_RETRIES ?? CONFIG_FALLBACK.VENICE_PROVIDER_MAX_RETRIES,
      };
    } catch {
      return CONFIG_FALLBACK;
    }
  }

  private resolveCircuitConfig(): CircuitEnvConfig {
    try {
      const config = getConfig() as any;
      return {
        VENICE_CIRCUIT_FAILURE_THRESHOLD:
          config?.VENICE_CIRCUIT_FAILURE_THRESHOLD ??
          CIRCUIT_CONFIG_FALLBACK.VENICE_CIRCUIT_FAILURE_THRESHOLD,
        VENICE_CIRCUIT_COOLDOWN_MS:
          config?.VENICE_CIRCUIT_COOLDOWN_MS ??
          CIRCUIT_CONFIG_FALLBACK.VENICE_CIRCUIT_COOLDOWN_MS,
        VENICE_CIRCUIT_PROBE_COUNT:
          config?.VENICE_CIRCUIT_PROBE_COUNT ??
          CIRCUIT_CONFIG_FALLBACK.VENICE_CIRCUIT_PROBE_COUNT,
      };
    } catch {
      return CIRCUIT_CONFIG_FALLBACK;
    }
  }

  private resolveBaseUrl(): string {
    try {
      return getConfig().VENICE_BASE_URL;
    } catch {
      return 'https://api.venice.ai/api/v1';
    }
  }

  getModelFor(agentType: AgentType): string {
    return MODEL_MAP[agentType];
  }

  /** Current breaker state: `CLOSED` | `OPEN` | `HALF_OPEN`. */
  getCircuitState(): CircuitState {
    return this.breaker.getState();
  }

  getFailureCount(): number {
    return this.breaker.getFailureCount();
  }

  /**
   * Full breaker observability (issue #495):
   * `{ state, failures, successes, lastFailureAt, lastSuccessAt }`.
   */
  getCircuitMetrics(): CircuitMetrics {
    return this.breaker.getMetrics();
  }

  /** The breaker itself, for callers that need {@link CircuitBreaker.on}. */
  getCircuitBreaker(): CircuitBreaker {
    return this.breaker;
  }

  /** Expose provider chain for observability / tests. */
  getProviders(): VeniceProviderConfig[] {
    return [...this.providers];
  }

  /** Current cache hit rate (0..1) for monitoring. */
  getCacheHitRate(): number {
    return this.cache.getHitRate();
  }

  /** Clear the entire response cache. */
  invalidateCache(): void {
    this.cache.invalidateAll();
  }

  /** Drop cache entries created under a specific model version. */
  invalidateModelVersion(modelVersion: string): void {
    this.cache.invalidateModelVersion(modelVersion);
  }

  async complete(
    prompt: string,
    agentType: AgentType,
    options?: CompleteOptions
  ): Promise<string> {
    const model = this.getModelFor(agentType);
    return this.createCompletion({
      messages: [{ role: 'user', content: prompt }],
      model,
      options,
      promptForLogging: prompt,
      agentType,
    });
  }

  async chat(messages: VeniceMessage[], options: VeniceChatOptions = {}): Promise<string> {
    const promptForLogging = messages.map(message => message.content).join('\n\n');
    return this.createCompletion({
      messages,
      model: options.model ?? DEFAULT_CHAT_MODEL,
      options,
      promptForLogging,
      agentType: 'chat',
    });
  }

  private async createCompletion({
    messages,
    model,
    options,
    promptForLogging,
    agentType,
  }: {
    messages: VeniceMessage[];
    model: string;
    options?: CompleteOptions;
    promptForLogging: string;
    agentType: string;
  }): Promise<string> {
    const maxTokens = options?.maxTokens ?? DEFAULT_MAX_TOKENS;
    if (maxTokens > HARD_TOKEN_CAP) {
      throw new TokenBudgetExceededError(maxTokens, HARD_TOKEN_CAP);
    }

    // Circuit breaker admission. In HALF_OPEN this reserves one of the probe
    // slots, so every exit path below must settle it exactly once — either
    // recordSuccess/recordFailure (from the fetch) or release() (when the call
    // never reached the network, e.g. a cache hit).
    let probeHeld = false;
    let settled = false;
    const settleSuccess = (): void => {
      if (settled) return;
      settled = true;
      this.breaker.recordSuccess();
    };
    const settleFailure = (): void => {
      if (settled) return;
      settled = true;
      this.breaker.recordFailure();
    };

    try {
      try {
        this.breaker.acquire();
        probeHeld = this.breaker.getState() === 'HALF_OPEN';
      } catch (e) {
        if (this.enableCacheFallback && !options?.force) {
          const stale = this.cache.getStale(promptForLogging, agentType, this.modelVersion);
          if (stale !== null) {
            log.warn({ agentType, model, circuitState: this.breaker.getState() }, 'venice circuit open — serving stale cache');
            return stale;
          }
        }
        throw e;
      }

      const force = options?.force === true;
      const cacheKey = buildCacheKey(promptForLogging, agentType, this.modelVersion);

      if (!force) {
        const cached = this.cache.get(promptForLogging, agentType, this.modelVersion);
        if (cached !== null) {
          log.info(
            { agentType, model, modelVersion: this.modelVersion, hitRate: this.cache.getHitRate() },
            'venice cache hit',
          );
          return cached;
        }
      }

      const runFetch = (): Promise<string> =>
        this.runVeniceFetch({ messages, model, options, promptForLogging, agentType, settleSuccess, settleFailure });

      let result: string;
      try {
        result = force ? await runFetch() : await this.deduplicator.dedup(cacheKey, runFetch);
      } catch (err) {
        // Graceful degradation: if all providers failed and we have stale cache, return it
        if (this.enableCacheFallback && !force) {
          const stale = this.cache.getStale(promptForLogging, agentType, this.modelVersion);
          if (stale !== null) {
            log.warn(
              { agentType, model, error: err instanceof Error ? err.message : String(err) },
              'venice all providers failed — serving stale cache (graceful degradation)',
            );
            return stale;
          }
        }
        throw err;
      }

      if (!force) {
        this.cache.set(promptForLogging, agentType, this.modelVersion, result);
      }
      return result;
    } finally {
      // Give back a HALF_OPEN probe slot the call never used. Harmless when the
      // slot was already settled — the breaker floors the counter at zero.
      if (probeHeld && !settled) {
        this.breaker.release();
      }
    }
  }

  private async runVeniceFetch({
    messages,
    model,
    options,
    promptForLogging,
    agentType,
    settleSuccess,
    settleFailure,
  }: {
    messages: VeniceMessage[];
    model: string;
    options?: CompleteOptions;
    promptForLogging: string;
    agentType: string;
    /** Records a success with the breaker (idempotent per call). */
    settleSuccess: () => void;
    /** Records a failure with the breaker (idempotent per call). */
    settleFailure: () => void;
  }): Promise<string> {
    const requestId = randomUUID();
    const start = Date.now();
    let retries = 0;

    const body = JSON.stringify({
      model,
      messages,
      temperature: options?.temperature ?? 0.2,
      max_tokens: options?.maxTokens ?? DEFAULT_MAX_TOKENS,
    });

    let lastError: Error | undefined;

    // Try providers in order (fallback chain)
    for (let pIndex = 0; pIndex < this.providers.length; pIndex++) {
      const provider = this.providers[pIndex]!;
      const isLastProvider = pIndex === this.providers.length - 1;

      try {
        const response = await this.fetchWithRetryForProvider(
          body,
          provider,
          () => { retries++; },
        );
        const data: unknown = await response.json();
        const content = (data as any)?.choices?.[0]?.message?.content;
        if (typeof content !== 'string') {
          throw new Error('Venice response missing expected content field');
        }

        settleSuccess();
        this.logRequest(requestId, agentType, model, promptForLogging, Date.now() - start, 'ok', retries, provider.name);
        return content;
      } catch (err) {
        if (err instanceof CircuitOpenError || err instanceof TokenBudgetExceededError) {
          throw err;
        }
        lastError = err instanceof Error ? err : new Error(String(err));
        // Non-retryable 400/422 on last provider should not failover further — but we still try next if available
        const isNonRetryable = lastError.message.includes('non-retryable');
        // For 401, trying next provider with different key may succeed, so we do failover
        if (pIndex < this.providers.length - 1) {
          const nextProvider = this.providers[pIndex + 1]!.name ?? `fallback-${pIndex + 1}`;
          log.warn(
            { agentType, model, failedProvider: provider.name, nextProvider, error: lastError.message, retries },
            'venice provider failed — failing over to next provider',
          );
          // small backoff before failover to next provider
          await this.sleep(100);
          continue;
        }
        // Last provider failed — record failure for circuit breaker
        settleFailure();
        this.logRequest(requestId, agentType, model, promptForLogging, Date.now() - start, 'error', retries, provider.name);
        // If we have stale cache fallback enabled, the caller (createCompletion) will handle it
        throw lastError;
      }
    }

    // Should not reach here, but fallback
    settleFailure();
    throw lastError ?? new Error('Venice AI is unreachable (all providers failed)');
  }

  async stream(
    prompt: string,
    agentType: AgentType,
    onChunk: (chunk: string) => void,
    options?: CompleteOptions
  ): Promise<void> {
    const maxTokens = options?.maxTokens ?? DEFAULT_MAX_TOKENS;
    if (maxTokens > HARD_TOKEN_CAP) {
      throw new TokenBudgetExceededError(maxTokens, HARD_TOKEN_CAP);
    }

    // Streams have no cache to fall back on, so an open circuit is shed with
    // CircuitOpenError. In HALF_OPEN a probe slot is reserved and settled by
    // exactly one of the recordSuccess/recordFailure calls below.
    let probeHeld = false;
    let settled = false;
    const settleSuccess = (): void => {
      if (settled) return;
      settled = true;
      this.breaker.recordSuccess();
    };
    const settleFailure = (): void => {
      if (settled) return;
      settled = true;
      this.breaker.recordFailure();
    };

    this.breaker.acquire();
    probeHeld = this.breaker.getState() === 'HALF_OPEN';

    const model = this.getModelFor(agentType);
    const requestId = randomUUID();
    const start = Date.now();
    let retries = 0;

    const body = JSON.stringify({
      model,
      messages: [{ role: 'user', content: prompt }],
      temperature: options?.temperature ?? 0.2,
      max_tokens: maxTokens,
      stream: true,
    });

    let accumulated = '';
    let lastError: Error | undefined;

    try {
      for (let pIndex = 0; pIndex < this.providers.length; pIndex++) {
        const provider = this.providers[pIndex]!;
        try {
          const response = await this.fetchWithRetryForProvider(body, provider, () => { retries++; });

          if (!response.body) {
            throw new Error('Venice stream response has no body');
          }

          const reader = response.body.getReader();
          const decoder = new TextDecoder();
          let done = false;

          while (!done) {
            const result = await reader.read();
            done = result.done;
            if (result.value) {
              const text = decoder.decode(result.value, { stream: !done });
              const lines = text.split('\n');
              for (const line of lines) {
                if (!line.startsWith('data: ')) continue;
                const payload = line.slice(6).trim();
                if (payload === '[DONE]') continue;
                try {
                  const parsed = JSON.parse(payload);
                  const delta = parsed?.choices?.[0]?.delta?.content;
                  if (typeof delta === 'string' && delta.length > 0) {
                    accumulated += delta;
                    onChunk(delta);
                  }
                } catch {
                  // skip malformed SSE chunks
                }
              }
            }
          }

          settleSuccess();
          this.logRequest(requestId, agentType, model, prompt, Date.now() - start, 'ok', retries, provider.name);
          return;
        } catch (err) {
          if (err instanceof CircuitOpenError || err instanceof TokenBudgetExceededError) {
            throw err;
          }
          lastError = err instanceof Error ? err : new Error(String(err));
          if (pIndex < this.providers.length - 1) {
            log.warn({ agentType, model, failedProvider: provider.name, error: lastError.message }, 'venice stream provider failed — failover');
            await this.sleep(100);
            continue;
          }
          settleFailure();
          this.logRequest(requestId, agentType, model, prompt, Date.now() - start, 'error', retries, provider.name);
          throw new Error(
            `Venice stream error after ${accumulated.length} characters accumulated: ${lastError.message}`,
          );
        }
      }

      settleFailure();
      throw lastError ?? new Error('Venice stream failed (all providers)');
    } finally {
      if (probeHeld && !settled) {
        this.breaker.release();
      }
    }
  }

  /**
   * Per-provider fetch with retries, exponential backoff and per-call timeout.
   */
  private async fetchWithRetryForProvider(
    body: string,
    provider: VeniceProviderConfig,
    onRetry: () => void
  ): Promise<Response> {
    let lastError: Error | undefined;
    const maxAttempts = Math.min(this.maxRetries, RETRY_DELAYS_MS.length) + 1;

    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      let timeoutId: ReturnType<typeof setTimeout> | undefined;
      const controller = new AbortController();
      if (this.timeoutMs > 0) {
        timeoutId = setTimeout(() => controller.abort(), this.timeoutMs);
      }

      try {
        const response = await fetch(`${provider.baseUrl ?? this.baseUrl}/chat/completions`, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            'Authorization': `Bearer ${provider.apiKey}`,
          },
          body,
          signal: controller.signal,
        });

        if (timeoutId) clearTimeout(timeoutId);

        if (response.ok) {
          return response;
        }

        if (NON_RETRYABLE_STATUS_CODES.has(response.status) && response.status !== 401) {
          // 401 may succeed on fallback with different key, so we treat it as retriable for failover
          throw new Error(`Venice returned non-retryable status: ${response.status}`);
        }

        // 401 is special: allow failover to next provider, not retry same provider
        if (response.status === 401) {
          throw new Error(`Venice returned non-retryable status: ${response.status}`);
        }

        if (RETRYABLE_STATUS_CODES.has(response.status) && attempt < maxAttempts - 1) {
          onRetry();
          await this.sleep(this.backoffDelay(attempt));
          continue;
        }

        throw new Error(`Venice returned status: ${response.status}`);
      } catch (err) {
        if (timeoutId) clearTimeout(timeoutId);
        // AbortError from timeout
        if (err instanceof Error && err.name === 'AbortError') {
          lastError = new Error(`Venice request timed out after ${this.timeoutMs}ms`);
          if (attempt < maxAttempts - 1) {
            onRetry();
            await this.sleep(this.backoffDelay(attempt));
            continue;
          }
          throw lastError;
        }
        if (err instanceof Error && err.message.startsWith('Venice returned')) {
          // For non-retryable, don't retry same provider — throw to allow failover to next provider
          throw err;
        }
        lastError = err instanceof Error ? err : new Error(String(err));
        if (attempt < maxAttempts - 1) {
          onRetry();
          await this.sleep(this.backoffDelay(attempt));
          continue;
        }
      }
    }

    throw lastError ?? new Error('Venice AI is unreachable');
  }

  private backoffDelay(attempt: number): number {
    const base = RETRY_DELAYS_MS[attempt] ?? 800;
    // Add jitter ±20% to avoid thundering herd
    const jitter = base * 0.2 * (Math.random() * 2 - 1);
    return Math.max(50, Math.round(base + jitter));
  }

  // Legacy fetchWithRetry kept for backward compat (delegates to primary provider)
  private async fetchWithRetry(
    body: string,
    onRetry: () => void
  ): Promise<Response> {
    const primary = this.providers[0];
    if (!primary) throw new Error('No Venice providers configured');
    return this.fetchWithRetryForProvider(body, primary, onRetry);
  }

  private sleep(ms: number): Promise<void> {
    return new Promise(resolve => setTimeout(resolve, ms));
  }

  private logRequest(
    veniceRequestId: string,
    agentType: string,
    model: string,
    prompt: string,
    durationMs: number,
    status: 'ok' | 'error',
    retries: number,
    providerName?: string
  ): void {
    const promptTokenEstimate = Math.ceil(prompt.length / 4);
    log.info({
      veniceRequestId,
      agentType,
      model,
      promptTokenEstimate,
      durationMs,
      status,
      retries,
      provider: providerName ?? 'primary',
      circuitState: this.breaker.getState(),
      promptPreview: prompt.slice(0, 200),
    }, 'venice request');
  }
}
