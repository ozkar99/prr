import { useCallback, useEffect, useState } from 'react'
import type { DiskUsage } from '../../shared/types'

const size = (bytes: number) =>
  bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : bytes >= 1e6 ? `${Math.round(bytes / 1e6)} MB` : `${Math.round(bytes / 1e3)} KB`

/**
 * Disk used by the app's own clones and PR checkouts, with buttons to clear
 * them. PR checkouts not opened for 14 days are also removed on startup.
 * PRs reviewed in the user's own clone use no extra space and are never touched.
 */
export function Storage() {
  const [usage, setUsage] = useState<DiskUsage>()
  const [clearing, setClearing] = useState(false)

  const refresh = useCallback(() => {
    void window.api.diskUsage().then(setUsage, () => {})
  }, [])
  useEffect(refresh, [refresh])

  const clear = async (includeClones: boolean) => {
    setClearing(true)
    try {
      await window.api.clearCheckouts(includeClones)
    } finally {
      setClearing(false)
      refresh()
    }
  }

  if (!usage || (usage.worktrees === 0 && usage.repos === 0)) return null
  return (
    <section className="storage">
      <h3>Storage</h3>
      <div className="storage-row">
        <span>
          {usage.worktrees} PR checkout{usage.worktrees === 1 ? '' : 's'} · {size(usage.worktreeBytes)}
          <span className="muted"> · {usage.repos} cloned repo{usage.repos === 1 ? '' : 's'} · {size(usage.repoBytes)}</span>
        </span>
        <span className="spacer" />
        <button className="button" disabled={clearing || usage.worktrees === 0} onClick={() => void clear(false)}>
          Clear PR checkouts
        </button>
        <button
          className="button"
          disabled={clearing}
          title="Also removes the cloned repos; the next PR from each repo clones it again"
          onClick={() => void clear(true)}
        >
          Clear everything
        </button>
      </div>
      <div className="muted storage-note">
        Checkouts not opened for 14 days are removed automatically. Run <code>review-tool</code> inside your own clone of a
        repo to review its PRs there, with no extra copy.
      </div>
    </section>
  )
}
