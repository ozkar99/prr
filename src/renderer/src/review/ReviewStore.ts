import { useSyncExternalStore } from 'react'
import type { DiffSide, PRDetails, ReviewEvent, ReviewState, ReviewThread } from '../../../shared/types'

export type ReviewSnapshot = {
  state?: ReviewState
  loading: boolean
  error?: string
}

/**
 * Review threads and the viewer's pending review for one PR. Every mutation
 * reloads from GitHub afterwards, so GitHub stays the single source of truth.
 */
export class ReviewStore {
  private snapshot: ReviewSnapshot = { loading: true }
  private listeners = new Set<() => void>()

  constructor(readonly pr: PRDetails) {}

  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  get = () => this.snapshot

  private set(patch: Partial<ReviewSnapshot>) {
    this.snapshot = { ...this.snapshot, ...patch }
    for (const l of this.listeners) l()
  }

  async load(): Promise<void> {
    this.set({ loading: true })
    try {
      const state = await window.api.reviewState(this.pr.owner, this.pr.repo, this.pr.number)
      this.set({ state, loading: false, error: undefined })
    } catch (e) {
      this.set({ loading: false, error: (e as Error).message })
    }
  }

  private async mutate(op: (state: ReviewState) => Promise<void>): Promise<void> {
    const state = this.snapshot.state
    if (!state) throw new Error('Review state not loaded')
    try {
      await op(state)
    } finally {
      await this.load()
    }
  }

  addThread(path: string, side: DiffSide, line: number, startLine: number | undefined, body: string) {
    return this.mutate((s) => window.api.addThread(s, this.pr.headSha, { path, side, line, startLine, body }))
  }

  reply(threadId: string, body: string) {
    return this.mutate((s) => window.api.reply(s, this.pr.headSha, threadId, body))
  }

  deleteComment(commentId: string) {
    return this.mutate(() => window.api.deleteComment(commentId))
  }

  setResolved(threadId: string, resolved: boolean) {
    return this.mutate(() => window.api.setResolved(threadId, resolved))
  }

  submit(event: ReviewEvent, body: string) {
    return this.mutate((s) => window.api.submitReview(s, this.pr.headSha, event, body))
  }
}

export function useReview(store: ReviewStore): ReviewSnapshot {
  return useSyncExternalStore(store.subscribe, store.get)
}

export const pendingCount = (state?: ReviewState) =>
  state?.threads.reduce((n, t) => n + t.comments.filter((c) => c.pending).length, 0) ?? 0

/** Threads anchored to a visible line of a file (outdated threads have no line). */
export const threadsAt = (state: ReviewState | undefined, path: string, side: DiffSide, line: number): ReviewThread[] =>
  state?.threads.filter((t) => t.path === path && t.side === side && t.line === line) ?? []
