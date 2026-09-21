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
    autoplay: { enabled: false, majsoul: { remaining_games: 1 } },
  } as AppConfig
}

describe('persistent autoplay controls', () => {
  beforeEach(() => {
    invoke.mockReset()
    let saved = config()
    useConfigStore.getState().setConfig(saved)
    invoke.mockImplementation(async (_command: string, patch: { enabled?: boolean; remaining_games?: number }) => {
      saved = {
        ...saved,
        autoplay: {
          ...saved.autoplay,
          enabled: patch.enabled ?? saved.autoplay.enabled,
          majsoul: {
            ...saved.autoplay.majsoul,
            remaining_games: patch.remaining_games ?? saved.autoplay.majsoul.remaining_games,
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
    await waitFor(() => expect(useConfigStore.getState().config?.autoplay.majsoul.remaining_games).toBe(3))
    expect(invoke).toHaveBeenLastCalledWith('update_autoplay_controls', { remaining_games: 3 })
  })

  it('allows zero and reflects backend countdown updates', async () => {
    render(<AutoplayControls />)
    useConfigStore.getState().setConfig({
      ...config(),
      autoplay: { ...config().autoplay, majsoul: { ...config().autoplay.majsoul, remaining_games: 2 } },
    })
    await waitFor(() => expect((screen.getByRole('spinbutton') as HTMLInputElement).value).toBe('2'))
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '0' } })
    fireEvent.blur(screen.getByRole('spinbutton'))
    await waitFor(() => expect(useConfigStore.getState().config?.autoplay.majsoul.remaining_games).toBe(0))
    expect(invoke).toHaveBeenLastCalledWith('update_autoplay_controls', { remaining_games: 0 })
  })

  it('does not overwrite a newer countdown with an older save response', async () => {
    let resolveSave!: (value: AppConfig) => void
    invoke.mockImplementationOnce(() => new Promise<AppConfig>((resolve) => { resolveSave = resolve }))
    render(<AutoplayControls />)
    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '3' } })
    fireEvent.blur(screen.getByRole('spinbutton'))
    await waitFor(() => expect(invoke).toHaveBeenCalled())
    const newer = config()
    newer.autoplay.majsoul.remaining_games = 2
    useConfigStore.getState().setConfig(newer)
    resolveSave({ ...newer, autoplay: { ...newer.autoplay, majsoul: { ...newer.autoplay.majsoul, remaining_games: 3 } } })
    await waitFor(() => expect((screen.getByRole('spinbutton') as HTMLInputElement).disabled).toBe(false))
    await waitFor(() => expect((screen.getByRole('spinbutton') as HTMLInputElement).value).toBe('2'))
    expect(useConfigStore.getState().config?.autoplay.majsoul.remaining_games).toBe(2)
  })

  it('keeps an in-progress edit when the countdown changes', async () => {
    render(<AutoplayControls />)
    const input = screen.getByRole('spinbutton') as HTMLInputElement
    fireEvent.focus(input)
    fireEvent.change(input, { target: { value: '5' } })
    const newer = config()
    newer.autoplay.majsoul.remaining_games = 2
    useConfigStore.getState().setConfig(newer)
    expect(input.value).toBe('5')
    fireEvent.blur(input)
    await waitFor(() => expect(useConfigStore.getState().config?.autoplay.majsoul.remaining_games).toBe(5))
  })
})
