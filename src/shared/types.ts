export type PRRef = { owner: string; repo: string; number: number }

/** Parses a PR URL or owner/repo#123. */
export function parsePRRef(input: string): PRRef | undefined {
  const s = input.trim()
  const m = s.match(/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/) ?? s.match(/^([\w.-]+)\/([\w.-]+)#(\d+)$/)
  return m ? { owner: m[1], repo: m[2], number: Number(m[3]) } : undefined
}

export type ToolStatus = {
  name: string
  purpose: string
  required: boolean
  ok: boolean
  problem: string | null
  fix: string | null
}

export type AgentInfo = { name: string; command: string | null; configPath: string }

/** The open PR, as the agent sees it through the get_pr / get_file_diff tools. */
export type ReviewContext = {
  root: string
  mergeBase: string
  pr: {
    repo: string
    number: number
    title: string
    url: string
    author: string
    baseBranch: string
    description: string
    files: { path: string; status: string; additions: number; deletions: number }[]
  }
}

/** What the user is looking at, as the agent sees it through get_selection. */
export type EditorSelection = {
  /** Repo-relative path, or absolute for files outside the checkout (e.g. the Go stdlib). */
  path: string
  /** 'new' = PR head, 'old' = before the PR (left side of a diff). */
  side: 'new' | 'old'
  changedInPR: boolean
  startLine: number
  endLine: number
  /** True when nothing is selected; `text` is then the cursor line. */
  cursorOnly: boolean
  text: string
}

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

export type LspLanguage = 'go' | 'cpp' | 'python' | 'starlark' | 'shell'

export type FileStat = { type: 'file' | 'directory'; size: number; mtime: number; ctime: number }

export type Api = {
  reviewQueue(): Promise<PRSummary[]>
  getPR(owner: string, repo: string, number: number): Promise<PRDetails>
  checkout(pr: PRDetails): Promise<Checkout>
  readBase(owner: string, repo: string, sha: string, path: string): Promise<string | null>
  fsStat(path: string): Promise<FileStat | null>
  fsRead(path: string): Promise<Uint8Array>
  fsReaddir(path: string): Promise<[string, 'file' | 'directory'][]>
  /** Starts a language server; `port` speaks LSP JSON-RPC messages. */
  startLsp(lang: LspLanguage, root: string): Promise<{ id: number; port: MessagePort }>
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

  /** Tells the agent integration which PR is open (null when none). */
  setReviewContext(review: ReviewContext | null): Promise<void>
  /** Reports what the user has selected, for the agent's get_selection tool. */
  setSelection(selection: EditorSelection | null): Promise<void>
  /** Which external tools (git, gh, language servers, agent) are available. */
  checkTools(): Promise<ToolStatus[]>
  /** The agent the panel runs: Claude Code, or the command in ~/.review-tool-agent-command. */
  agentInfo(): Promise<AgentInfo>
  /** Starts the agent in a terminal; returns the terminal id. */
  agentStart(
    opts: { root: string; prLabel: string; mergeBase: string; cols: number; rows: number },
    onData: (bytes: Uint8Array) => void
  ): Promise<number>
  ptyWrite(id: number, data: string): Promise<void>
  ptyResize(id: number, cols: number, rows: number): Promise<void>
  ptyKill(id: number): Promise<void>
  onPtyExit(cb: (id: number) => void): () => void
  /** A PR the app was launched with (`review-tool <pr>`), consumed once. */
  takeLaunchPR(): Promise<PRRef | null>
  /** PRs sent by later `review-tool <pr>` calls while the app is running. */
  onOpenPR(cb: (ref: PRRef) => void): () => void
  onProgress(cb: (message: string) => void): () => void
  onLspExit(cb: (info: { id: number; lang: LspLanguage; code: number | null; stderrTail: string }) => void): () => void
}
