export const HAS_BACKEND = true

type Envelope<T> = {
  ok: boolean
  value?: T
  error?: unknown
  message?: string
}

export async function invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  const response = await fetch(`/api/invoke/${encodeURIComponent(cmd)}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(args),
  })
  const envelope = (await response.json()) as Envelope<T>
  if (!response.ok || !envelope.ok) {
    const error = new Error(envelope.message ?? `request failed: ${response.status}`)
    if (envelope.error && typeof envelope.error === 'object') Object.assign(error, envelope.error)
    throw error
  }
  return envelope.value as T
}

type Listener = (payload: unknown) => void
const listeners = new Map<string, Set<Listener>>()
let source: EventSource | null = null

function eventSource(): EventSource {
  if (source) return source
  source = new EventSource('/api/events')
  const names = [
    'mjai-event', 'bot-response', 'bot-status', 'capture-status', 'notify',
    'analysis-result', 'history-recorded', 'log-entry', 'inspector-entry', 'config-updated',
  ]
  for (const name of names) {
    source.addEventListener(name, (event) => {
      const payload = JSON.parse((event as MessageEvent<string>).data) as unknown
      listeners.get(name)?.forEach((listener) => listener(payload))
    })
  }
  return source
}

export async function listen<T>(name: string, callback: (payload: T) => void): Promise<() => void> {
  eventSource()
  const set = listeners.get(name) ?? new Set<Listener>()
  const listener: Listener = (payload) => callback(payload as T)
  set.add(listener)
  listeners.set(name, set)
  return () => {
    set.delete(listener)
    if (set.size === 0) listeners.delete(name)
  }
}

export async function installBotFromZip(archive: File, name?: string): Promise<void> {
  const form = new FormData()
  form.append('archive', archive)
  if (name) form.append('name', name)
  const response = await fetch('/api/install-bot', { method: 'POST', body: form })
  const envelope = (await response.json()) as Envelope<unknown>
  if (!response.ok || !envelope.ok) throw new Error(envelope.message ?? 'install failed')
}
