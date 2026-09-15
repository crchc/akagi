import { useEffect, useRef } from 'react'
import { listen } from '@/lib/api'
import { useLogsStore } from '@/stores/logsStore'
import type { LogEntry } from '@/types'

/**
 * Live-tail subscription to the active session's tracing stream.
 *
 * Active only when we're viewing the active session and the user hasn't
 * paused via the toggle. Arrivals are
 * buffered in a ref and flushed via `requestAnimationFrame` (~60 Hz cap)
 * so React never re-renders at event rate — at `RUST_LOG=trace` under
 * proxy load that can be thousands of events per second, and a naive
 * `setEntries([...prev, ev])` would lock up the page.
 *
 * The shared SSE connection is filtered through one local listener per
 * subscribed render and removed on cleanup.
 */
export function useLogStream(enabled: boolean = true): void {
  const isLive = useLogsStore((s) => s.isLive)
  const currentSession = useLogsStore((s) => s.currentSession)
  const activeSession = useLogsStore((s) => s.activeSession)
  const appendBatch = useLogsStore((s) => s.appendBatch)

  const bufferRef = useRef<LogEntry[]>([])
  const rafRef = useRef<number | null>(null)

  useEffect(() => {
    if (!enabled) return
    if (!isLive) return
    if (!currentSession || !activeSession) return
    if (currentSession !== activeSession) return

    let cancelled = false

    const flush = () => {
      rafRef.current = null
      const buf = bufferRef.current
      if (buf.length === 0) return
      bufferRef.current = []
      appendBatch(buf)
    }

    let unlisten: (() => void) | undefined
    listen<LogEntry>('log-entry', (entry) => {
      if (cancelled) return
      bufferRef.current.push(entry)
      if (rafRef.current == null) {
        rafRef.current = requestAnimationFrame(flush)
      }
    }).then((stop) => { unlisten = stop })

    return () => {
      cancelled = true
      unlisten?.()
      if (rafRef.current != null) {
        cancelAnimationFrame(rafRef.current)
        rafRef.current = null
      }
      bufferRef.current = []
    }
  }, [enabled, isLive, currentSession, activeSession, appendBatch])
}
