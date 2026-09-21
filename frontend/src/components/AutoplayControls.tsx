import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { invoke } from '@/lib/api'
import { useConfigStore } from '@/stores/configStore'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import type { AppConfig } from '@/types'

export function AutoplayControls() {
  const { t } = useTranslation()
  const config = useConfigStore((s) => s.config)
  const setConfig = useConfigStore((s) => s.setConfig)
  const [busy, setBusy] = useState(false)
  const [warningOpen, setWarningOpen] = useState(false)
  const pending = useRef<Promise<unknown>>(Promise.resolve())
  const pendingCount = useRef(0)
  const totalInput = useRef<HTMLInputElement>(null)

  const total = config?.autoplay.majsoul.total_games ?? 1

  if (!config) return null

  const enabled = config.autoplay.enabled
  const isMajsoul = config.platform.kind === 'Majsoul'
  const isRiichiCity = config.platform.kind === 'RiichiCity'
  const canEnable = isRiichiCity || config.capture.mode === 'chromium'

  const save = (patch: { enabled?: boolean; total_games?: number }) => {
    pendingCount.current += 1
    setBusy(true)
    const task = pending.current.then(() => invoke<AppConfig>('update_autoplay_controls', patch))
    pending.current = task.then(() => undefined, () => undefined)
    void task.then(setConfig).catch((error: unknown) => {
      if (totalInput.current) {
        totalInput.current.value = String(useConfigStore.getState().config?.autoplay.majsoul.total_games ?? 1)
      }
      toast.error(String(error))
    }).finally(() => {
      pendingCount.current -= 1
      if (pendingCount.current === 0) setBusy(false)
    })
  }

  const commitTotal = () => {
    const input = totalInput.current
    if (!input) return
    const value = Number(input.value)
    if (!Number.isSafeInteger(value) || value < 1) {
      input.value = String(total)
      return
    }
    if (value !== total) save({ total_games: value })
  }

  return (
    <div className="flex items-center gap-3 text-foreground">
      <label className="flex items-center gap-2 whitespace-nowrap">
        <span>{t('settings.autoplay.enable')}</span>
        <Switch
          size="sm"
          checked={enabled}
          disabled={busy || (!canEnable && !enabled)}
          aria-label={t('settings.autoplay.enable')}
          title={!canEnable ? t('settings.autoplay.requires_chromium') : undefined}
          onCheckedChange={(checked) => {
            if (checked && isRiichiCity) setWarningOpen(true)
            else save({ enabled: checked })
          }}
        />
      </label>
      {isMajsoul && (
        <label className="flex items-center gap-2 whitespace-nowrap" title={t('settings.autoplay.total_games_hint')}>
          <span>{t('settings.autoplay.total_games')}</span>
          <Input
            key={total}
            ref={totalInput}
            type="number"
            min={1}
            step={1}
            inputMode="numeric"
            className="w-16"
            defaultValue={total}
            disabled={busy}
            onBlur={commitTotal}
            onKeyDown={(event) => {
              if (event.key === 'Enter') event.currentTarget.blur()
            }}
          />
        </label>
      )}
      <Dialog open={warningOpen} onOpenChange={setWarningOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('settings.autoplay.rc_warning_title')}</DialogTitle>
            <DialogDescription>{t('settings.autoplay.rc_warning_desc')}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button onClick={() => {
              setWarningOpen(false)
              save({ enabled: true })
            }}>
              {t('settings.autoplay.rc_warning_ack')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
