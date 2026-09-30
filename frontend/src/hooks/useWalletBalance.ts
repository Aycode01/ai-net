import { useState, useEffect, useRef, useCallback } from 'react'

const HORIZON_URL = 'https://horizon-testnet.stellar.org'
const POLL_INTERVAL = 10_000

export interface WalletBalance {
  asset_type: 'native' | 'credit_alphanum4' | 'credit_alphanum12'
  asset_code?: string
  asset_issuer?: string
  balance: string
}

interface BalanceInfo {
  /** Native XLM balance (kept for backward compatibility). */
  balance: string
  /** Full set of on-chain balances including native XLM and issued tokens. */
  balances: WalletBalance[]
  loading: boolean
  error: string | null
}

/**
 * Fetches the full balance set for a Stellar account from Horizon. The native
 * XLM balance is always present for a funded account; additional trustlines
 * surface as `credit_alphanum4`/`credit_alphanum12` issued-asset entries.
 *
 * Wallet-switch behaviour: changing `publicKey` immediately clears stale data,
 * sets loading to true, and triggers a fresh fetch without waiting for the next
 * poll interval. No ref is mutated during the render phase.
 */
export function useWalletBalance(publicKey: string | null): BalanceInfo {
  const [balance, setBalance] = useState<string>('0')
  const [balances, setBalances] = useState<WalletBalance[]>([])
  const [loading, setLoading] = useState<boolean>(true)
  const [error, setError] = useState<string | null>(null)
  const isFirstLoad = useRef(true)

  // Clear stale data immediately when the wallet changes so the UI never
  // shows the previous wallet's numbers while the new fetch is in-flight.
  useEffect(() => {
    setBalance('0')
    setBalances([])
    setError(null)
    setLoading(true)
    isFirstLoad.current = true
  }, [publicKey])

  // `publicKey` is captured directly in the callback so a wallet switch
  // produces a new function reference and re-triggers the fetch effect below.
  const fetchBalance = useCallback(async () => {
    if (!publicKey) {
      setBalance('0')
      setBalances([])
      setError(null)
      setLoading(false)
      return
    }

    // Only show loading indicator on the first fetch, not on subsequent polls
    if (isFirstLoad.current) {
      setLoading(true)
    }
    try {
      const res = await fetch(`${HORIZON_URL}/accounts/${publicKey}`)
      if (!res.ok) {
        if (res.status === 404) {
          setBalance('0')
          setBalances([])
          setError(null)
          return
        }
        throw new Error(`Horizon error: ${res.status}`)
      }
      const data = await res.json()
      const rawBalances: WalletBalance[] = Array.isArray(data.balances) ? data.balances : []
      const xlmBalance = rawBalances.find((b) => b.asset_type === 'native')
      setBalance(xlmBalance?.balance ?? '0')
      setBalances(rawBalances)
      setError(null)
      isFirstLoad.current = false
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to fetch balance'
      setError(message)
    } finally {
      setLoading(false)
    }
  }, [publicKey])

  useEffect(() => {
    fetchBalance()

    const intervalId = setInterval(fetchBalance, POLL_INTERVAL)

    return () => {
      clearInterval(intervalId)
    }
  }, [fetchBalance])

  return { balance, balances, loading, error }
}
