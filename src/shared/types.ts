export type PRSummary = {
  owner: string
  repo: string
  number: number
  title: string
  author: string
  updatedAt: string
  draft: boolean
  reason: 'review-requested' | 'authored'
}

export type FileStatus = 'added' | 'removed' | 'modified' | 'renamed' | 'copied' | 'changed' | 'unchanged'

/** Inclusive line ranges covered by the diff hunks; GitHub only accepts comments on these. */
export type CommentableRanges = { LEFT: [number, number][]; RIGHT: [number, number][] }

export type PRFile = {
  path: string
  previousPath?: string
  status: FileStatus
  additions: number
  deletions: number
  /** Missing when GitHub omits the patch (very large or binary files). */
  commentable?: CommentableRanges
}

export type DiffSide = 'LEFT' | 'RIGHT'

export type ReviewComment = {
  id: string
  author: string
  authorAvatar: string
  body: string
  createdAt: string
  url: string
  /** Part of the viewer's unsubmitted review. */
  pending: boolean
  viewerCanDelete: boolean
}

export type ReviewThread = {
  id: string
  path: string
  side: DiffSide
  /** Current line on `side`; null when the thread is outdated. */
  line: number | null
  startLine: number | null
  isResolved: boolean
  isOutdated: boolean
  viewerCanResolve: boolean
  viewerCanUnresolve: boolean
  comments: ReviewComment[]
}

/** Something in the PR's conversation that isn't attached to a file. */
export type ConversationItem = {
  kind: 'comment' | 'review'
  id: string
  author: string
  authorAvatar: string
  body: string
  createdAt: string
  url: string
  /** Review outcome, for kind 'review'. */
  reviewState?: 'APPROVED' | 'CHANGES_REQUESTED' | 'COMMENTED' | 'DISMISSED'
}

export type ReviewState = {
  prNodeId: string
  viewer: string
  /** The viewer's unsubmitted review, if any. */
  pendingReviewId?: string
  threads: ReviewThread[]
  /** General PR comments and submitted reviews, oldest first. */
  conversation: ConversationItem[]
}

export type ReviewEvent = 'COMMENT' | 'APPROVE' | 'REQUEST_CHANGES'

export type NewThread = { path: string; side: DiffSide; line: number; startLine?: number; body: string }

export type PRDetails = {
  owner: string
  repo: string
  number: number
  title: string
  body: string
  author: string
  url: string
  baseRef: string
  baseSha: string
  headSha: string
  files: PRFile[]
}

export type Checkout = {
  /** Absolute path of the worktree checked out at the PR head. */
  root: string
  /** Commit the diff is computed against (merge-base of base and head, like GitHub). */
  mergeBase: string
  headSha: string
}

export type LspLanguage = 'go' | 'cpp'

export type FileStat = { type: 'file' | 'directory'; size: number; mtime: number; ctime: number }

export type Api = {
  reviewQueue(): Promise<PRSummary[]>
  getPR(owner: string, repo: string, number: number): Promise<PRDetails>
  checkout(pr: PRDetails): Promise<Checkout>
  readBase(owner: string, repo: string, sha: string, path: string): Promise<string | null>
  fsStat(path: string): Promise<FileStat | null>
  fsRead(path: string): Promise<Uint8Array>
  fsReaddir(path: string): Promise<[string, 'file' | 'directory'][]>
  /** Starts a server and returns its id; its MessagePort arrives as an 'lsp:port' window message. */
  startLsp(lang: LspLanguage, root: string): Promise<number>
  stopLsp(id: number): Promise<void>
  reviewState(owner: string, repo: string, number: number): Promise<ReviewState>
  /** Adds a comment thread to the viewer's pending review, creating the review if needed. */
  addThread(state: ReviewState, headSha: string, thread: NewThread): Promise<void>
  reply(state: ReviewState, headSha: string, threadId: string, body: string): Promise<void>
  deleteComment(commentId: string): Promise<void>
  /** Posts a general (not file-attached) comment on the PR. Publishes immediately. */
  addPRComment(prNodeId: string, body: string): Promise<void>
  setResolved(threadId: string, resolved: boolean): Promise<void>
  submitReview(state: ReviewState, headSha: string, event: ReviewEvent, body: string): Promise<void>
  openExternal(url: string): Promise<void>
  onProgress(cb: (message: string) => void): () => void
  onLspExit(cb: (info: { id: number; lang: LspLanguage; code: number | null; stderrTail: string }) => void): () => void
}
