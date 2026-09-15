import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { createHashRouter, redirect, RouterProvider } from 'react-router-dom'
import 'mahgen'
import './index.css'
import './i18n'
import './stores/themeStore'
import App from './App.tsx'
import { Overview } from '@/routes/Overview'
import { GameDashboard } from '@/routes/GameDashboard'
import { Bots } from '@/routes/Bots'
import { History } from '@/routes/History'
import { Review } from '@/routes/Review'
import { Logs } from '@/routes/Logs'
import { Settings } from '@/routes/Settings'
import { Setup } from '@/routes/Setup'
import { invoke } from '@/lib/api'
import type { AppConfig } from '@/types'

// Loader on the protected branch: bounce to /setup when first_run_completed
// is false. The /setup route lives outside this loader so it can render
const requireFirstRunCompleted = async () => {
  try {
    const cfg = await invoke<AppConfig>('get_config')
    if (!cfg.general.first_run_completed) {
      return redirect('/setup')
    }
  } catch {
    // If get_config fails the rest of the UI surfaces it; don't gate on it.
  }
  return null
}

const router = createHashRouter([
  { path: '/setup', element: <Setup /> },
  {
    element: <App />,
    loader: requireFirstRunCompleted,
    children: [
      { index: true, element: <Overview /> },
      { path: 'game', element: <GameDashboard /> },
      { path: 'bots', element: <Bots /> },
      { path: 'history', element: <History /> },
      { path: 'review', element: <Review /> },
      { path: 'logs', element: <Logs /> },
      { path: 'settings', element: <Settings /> },
    ],
  },
])

const root = createRoot(document.getElementById('root')!)
root.render(
  <StrictMode>
    <RouterProvider router={router} />
  </StrictMode>,
)
