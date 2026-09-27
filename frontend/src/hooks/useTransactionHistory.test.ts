import { renderHook, act } from '@testing-library/react'
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import {
  useTransactionHistory,
  clearMemoCache,
} from './useTransactionHistory'

describe('useTransactionHistory', () => {
  const originalFetch = global.fetch

  beforeEach(() => {
    clearMemoCache()
    vi.useFakeTimers()
  })

  afterEach(() => {
    global.fetch = originalFetch
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('caches memos by transaction hash and does not refetch known hashes on subsequent polls', async () => {
    const fetchMock = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/payments?')) {
        return Promise.resolve({
          ok: true,
          json: () =>
            Promise.resolve({
              _embedded: {
                records: [
                  {
                    amount: '10.0',
                    from: 'G_SENDER',
                    to: 'G_RECV',
                    transaction_hash: 'tx_hash_1',
                    created_at: '2026-01-01T00:00:00Z',
                    type: 'payment',
                  },
                ],
              },
            }),
        })
      }
      if (url.includes('/transactions/tx_hash_1')) {
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve({ memo: 'Test Memo 1' }),
        })
      }
      return Promise.reject(new Error('Unknown endpoint'))
    })

    global.fetch = fetchMock as any

    renderHook(() => useTransactionHistory('G_RECV'))

    await act(async () => {
      await Promise.resolve()
    })

    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/accounts/G_RECV/payments')
    )
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/transactions/tx_hash_1')
    )

    const initialFetchCount = fetchMock.mock.calls.length
    expect(initialFetchCount).toBe(2)

    // Advance 30 seconds for next poll
    await act(async () => {
      vi.advanceTimersByTime(30_000)
      await Promise.resolve()
    })

    // Payment endpoint is called again, but memo endpoint is NOT re-called!
    const memoFetchCalls = fetchMock.mock.calls.filter((call) =>
      call[0].includes('/transactions/')
    )
    expect(memoFetchCalls.length).toBe(1)
  })

  it('pauses polling when tab is hidden and stops on unmount', async () => {
    const fetchMock = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/payments?')) {
        return Promise.resolve({
          ok: true,
          json: () => Promise.resolve({ _embedded: { records: [] } }),
        })
      }
      return Promise.reject(new Error('Unknown endpoint'))
    })

    global.fetch = fetchMock as any

    const { unmount } = renderHook(() => useTransactionHistory('G_RECV'))

    await act(async () => {
      await Promise.resolve()
    })

    const initialCallCount = fetchMock.mock.calls.length

    // Simulate tab hidden
    Object.defineProperty(document, 'hidden', {
      configurable: true,
      get: () => true,
    })

    await act(async () => {
      vi.advanceTimersByTime(60_000)
      await Promise.resolve()
    })

    // No new calls while hidden
    expect(fetchMock.mock.calls.length).toBe(initialCallCount)

    // Unmount
    unmount()

    // Simulate tab visible again
    Object.defineProperty(document, 'hidden', {
      configurable: true,
      get: () => false,
    })

    await act(async () => {
      vi.advanceTimersByTime(60_000)
      await Promise.resolve()
    })

    // Still no calls because hook is unmounted
    expect(fetchMock.mock.calls.length).toBe(initialCallCount)
  })
})
