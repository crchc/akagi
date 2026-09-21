import { useEffect } from 'react'
import { invoke, listen } from '@/lib/api'
import type { AnalysisResult, AppConfig, BotResponse, BotStatus, CaptureStatus, GameRecord, GameStateSnapshot, HistoryEvent, MahgenView, MjaiEvent, Notification, Snapshot } from '@/types'
import { useGameStore } from '@/stores/gameStore'
import { useAnalysisStore } from '@/stores/analysisStore'
import { useBotStore } from '@/stores/botStore'
import { useCaptureStore } from '@/stores/captureStore'
import { useNotifyStore } from '@/stores/notifyStore'
import { useApiStatusStore } from '@/stores/apiStatusStore'
import { useInstallStore } from '@/stores/installStore'
import { useConfigStore } from '@/stores/configStore'
import { useHistoryStore } from '@/stores/historyStore'
import { toast, type ToastSeverity } from '@/components/ui/sonner'

const TOAST_SEVERITY: Record<Notification['level'], ToastSeverity> = {
  info: 'info', success: 'success', warn: 'warning', error: 'error',
}

export function useBackendBridge() {
  useEffect(() => {
    const unlistens: Array<() => void> = []
    let cancelled = false
    const refreshGame = async () => {
      try {
        const [snap, view] = await Promise.all([
          invoke<GameStateSnapshot | null>('get_game_snapshot'),
          invoke<MahgenView | null>('get_mahgen_view'),
        ])
        if (!cancelled) {
          useGameStore.getState().setGame(snap)
          useGameStore.getState().setView(view)
        }
      } catch { /* backend is still starting */ }
    }
    void (async () => {
      try {
        const status = await invoke<Snapshot>('get_status')
        if (cancelled) return
        useConfigStore.getState().setConfig(status.config)
        useConfigStore.getState().setLogDir(status.log_dir)
        useBotStore.getState().setStatus(status.bot_status)
        useCaptureStore.getState().set(status.capture_status)
        await refreshGame()
        useAnalysisStore.getState().set(await invoke<AnalysisResult | null>('get_analysis'))
        useHistoryStore.getState().setRecords(await invoke<GameRecord[]>('list_game_history', { filter: null, limit: 1_000_000, offset: 0 }))
      } catch { /* individual pages surface unavailable data */ }
    })()
    listen<MjaiEvent>('mjai-event', (event) => {
      if (event.type === 'start_game') useApiStatusStore.getState().reset()
      useNotifyStore.getState().pushEvent(event)
      void refreshGame()
    }).then((u) => unlistens.push(u))
    listen<AnalysisResult>('analysis-result', (v) => useAnalysisStore.getState().set(v)).then((u) => unlistens.push(u))
    listen<AppConfig>('config-updated', (v) => useConfigStore.getState().setConfig(v)).then((u) => unlistens.push(u))
    listen<BotStatus>('bot-status', (v) => useBotStore.getState().setStatus(v)).then((u) => unlistens.push(u))
    listen<CaptureStatus>('capture-status', (v) => useCaptureStore.getState().set(v)).then((u) => unlistens.push(u))
    listen<BotResponse>('bot-response', (v) => useNotifyStore.getState().pushResponse(v)).then((u) => unlistens.push(u))
    listen<HistoryEvent>('history-recorded', (v) => {
      if (v.kind === 'recorded') useHistoryStore.getState().prepend(v.record)
      else useHistoryStore.getState().remove(v.id)
    }).then((u) => unlistens.push(u))
    listen<Notification>('notify', (n) => {
      useNotifyStore.getState().pushToast(n)
      if (n.id === 'native-api-health') useApiStatusStore.getState().setDegraded(n.level === 'warn' || n.level === 'error', n.body)
      if (n.id && (n.id.startsWith('bot-install-') || n.id.startsWith('bot-sync-'))) useInstallStore.getState().setProgress(n)
      toast[TOAST_SEVERITY[n.level]](n.title, { description: n.body, id: n.id, ...(n.sticky ? { duration: Infinity } : {}) })
    }).then((u) => unlistens.push(u))
    return () => { cancelled = true; unlistens.forEach((u) => u()) }
  }, [])
}
