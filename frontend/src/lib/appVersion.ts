// scripts/tag_release.sh rewrites this exact declaration.
export const VERSION_FALLBACK = '3.7.1'

export async function getAppVersion(): Promise<string> {
  return VERSION_FALLBACK
}

/**
 * Compare two Akagi version strings (`3.5.0`, and the legacy `3.0.0-8`
 * beta shape) segment-wise: split on `.` and `-`, compare numerically,
 * treat missing segments as 0. Returns <0 / 0 / >0 like a comparator.
 * Non-numeric segments (never produced by tag_release.sh) fall back to
 * string comparison so the order is still total.
 */
export function compareVersions(a: string, b: string): number {
  const as = a.split(/[.-]/)
  const bs = b.split(/[.-]/)
  const len = Math.max(as.length, bs.length)
  for (let i = 0; i < len; i++) {
    const ra = as[i] ?? '0'
    const rb = bs[i] ?? '0'
    const na = Number(ra)
    const nb = Number(rb)
    if (Number.isFinite(na) && Number.isFinite(nb)) {
      if (na !== nb) return na < nb ? -1 : 1
    } else if (ra !== rb) {
      return ra < rb ? -1 : 1
    }
  }
  return 0
}
