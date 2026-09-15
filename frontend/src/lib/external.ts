import { invoke } from '@/lib/api'

// Project-wide community / source links. Used by the first-run wizard
// banners and the sidebar footer; kept here so the URLs aren't
// duplicated across components.
export const AKAGI_GITHUB_URL = 'https://github.com/shinkuan/Akagi'
export const AKAGI_DISCORD_URL = 'https://discord.gg/Z2wjXUK8bN'
export const AKAGIMS_GITHUB_URL = 'https://github.com/shinkuan/AkagiMS'
export const AKAGIMS_DOWNLOAD_URL = 'https://github.com/shinkuan/AkagiMS/releases/latest'

export function openExternal(url: string): void {
  invoke('open_external_url', { url }).catch(() => {})
}
