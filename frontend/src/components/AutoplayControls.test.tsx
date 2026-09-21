import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { AutoplayControls } from './AutoplayControls'
import { useConfigStore } from '@/stores/configStore'
import type { AppConfig } from '@/types'

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}))

const invoke = vi.fn()
vi.mock('@/lib/api', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))

function config(): AppConfig {
  return {
    platform: { kind: 'Majsoul' },
    capture: { mode: 'chromium' },
    autoplay: { enabled: false, majsoul: { total_games: 1 } },
  } as AppConfig
}

describe('persistent autoplay controls', () => {
  beforeEach(() => {
    invoke.mockReset()
    let saved = config()
    useConfigStore.getState().setConfig(saved)
    invoke.mockImplementation(async (_command: string, patch: { enabled?: boolean; total_games?: number }) => {
      saved = {
        ...saved,
        autoplay: {
          ...saved.autoplay,
          enabled: patch.enabled ?? saved.autoplay.enabled,
          majsoul: {
            ...saved.autoplay.majsoul,
            total_games: patch.total_games ?? saved.autoplay.majsoul.total_games,
          },
        },
      }
      return saved
    })
  })

  it('saves only the changed control and reflects the returned config', async () => {
    render(<AutoplayControls />)
    fireEvent.click(screen.getByRole('switch'))
    await waitFor(() => expect(useConfigStore.getState().config?.autoplay.enabled).toBe(true))
    expect(invoke).toHaveBeenCalledWith('update_autoplay_controls', { enabled: true })

    const total = screen.getByRole('spinbutton')
    fireEvent.change(total, { target: { value: '3' } })
    fireEvent.blur(total)
    await waitFor(() => expect(useConfigStore.getState().config?.autoplay.majsoul.total_games).toBe(3))
    expect(invoke).toHaveBeenLastCalledWith('update_autoplay_controls', { total_games: 3 })
  })
})
