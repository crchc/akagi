import { useEffect, useRef } from 'react'
import { listen } from '@/lib/api'
import { useInspectorStore } from '@/stores/inspectorStore'
import type { InspectorEntry } from '@/types'

/**
 * Live-tail subscription to the inspector pipeline.
 *
 * Same rAF-batching trick as `useLogStream`: WS frame storms during a
 * busy game can exceed React's render budget, so arrivals are coalesced
 * in a `useRef` buffer and flushed once per animation frame.
 *
 * `enabled` is the gate — caller turns this off when viewing a past
 * session, when the inspector tab isn't active, or when the user has
 * paused the live tail.
 */
export function useInspectorStream(enabled: boolean): void {
  const appendBatch = useInspectorStore((s) => s.appendBatch)

  const bufferRef = useRef<InspectorEntry[]>([])
  const rafRef = useRef<number | null>(null)

  useEffect(() => {
    if (!enabled) return

    let cancelled = false

    const flush = () => {
      rafRef.current = null
      const buf = bufferRef.current
      if (buf.length === 0) return
      bufferRef.current = []
      appendBatch(buf)
    }

    let unlisten: (() => void) | undefined
    listen<InspectorEntry>('inspector-entry', (entry) => {
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
  }, [enabled, appendBatch])
}
