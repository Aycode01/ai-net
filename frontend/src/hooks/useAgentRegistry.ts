import { useCallback, useEffect, useRef, useState } from 'react'
import { getAgentsPage } from '../services/api'
import type { AgentRecord } from '../types/api'
import { normalizeAgent } from '@utils/agentRegistry'

const REFRESH_INTERVAL = 30_000

export interface AgentRegistryOptions {
  refreshInterval?: number
}

export interface AgentRegistryResult {
  agents: AgentRecord[]
  loading: boolean
  error: string | null
  refetch: () => void
  nextPage: () => void
  firstPage: () => void
  hasNextPage: boolean
  isFirstPage: boolean
  fetching: boolean
}

/**
 * Fetches the agent registry from `GET /api/agents` and keeps it fresh with a
 * background poll and window focus listener. `loading` is only true on the first
 * load — subsequent refreshes (auto or manual) update `agents` in place so the
 * table never remounts and the skeleton never flashes.
 */
export function useAgentRegistry(options: AgentRegistryOptions = {}): AgentRegistryResult {
  const refreshInterval = options.refreshInterval ?? REFRESH_INTERVAL
  const [agents, setAgents] = useState<AgentRecord[]>([])
  const [loading, setLoading] = useState<boolean>(true)
  const [error, setError] = useState<string | null>(null)

  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [isFirstPage, setIsFirstPage] = useState(true)
  const [fetching, setFetching] = useState(false)
  const currentCursor = useRef<string | undefined>(undefined)
  const isFirstLoad = useRef(true)
  const fetchingRef = useRef(false)
  const mountedRef = useRef(true)

  const fetchAgents = useCallback(async (requestedCursor: string | null | undefined = currentCursor.current) => {
    const cursor = requestedCursor ?? undefined
    if (fetchingRef.current) return
    fetchingRef.current = true
    setFetching(true)

    if (isFirstLoad.current) setLoading(true)

    try {
      const data = await getAgentsPage(cursor)
      if (!mountedRef.current) return
      currentCursor.current = cursor
      setIsFirstPage(cursor === undefined)
      setNextCursor(data.pagination.nextCursor)
      setAgents(data.items.map(normalizeAgent))
      setError(null)
    } catch (err) {
      if (!mountedRef.current) return
      setError(err instanceof Error ? err.message : 'Failed to load agents')
    } finally {
      if (mountedRef.current) {
        setLoading(false)
        setFetching(false)
      }
      isFirstLoad.current = false
      fetchingRef.current = false
    }
  }, [])

  useEffect(() => {
    mountedRef.current = true
    fetchAgents()

    const interval = setInterval(fetchAgents, refreshInterval)
    const handleFocus = () => fetchAgents()
    if (typeof window !== 'undefined') {
      window.addEventListener('focus', handleFocus)
    }

    return () => {
      mountedRef.current = false
      clearInterval(interval)
      if (typeof window !== 'undefined') {
        window.removeEventListener('focus', handleFocus)
      }
    }
  }, [fetchAgents, refreshInterval])


  const refetch = useCallback(() => {
    fetchAgents()
  }, [fetchAgents])

  return {
    agents, loading, error, refetch, fetching, isFirstPage,
    hasNextPage: nextCursor !== null,
    nextPage: () => { if (nextCursor) void fetchAgents(nextCursor) },
    firstPage: () => { void fetchAgents(null) },
  }
}
