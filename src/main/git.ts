import { app } from 'electron'
import { existsSync } from 'node:fs'
import { mkdir } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import type { Checkout, PRDetails } from '../shared/types'
import { run } from './exec'

const reposDir = () => join(app.getPath('userData'), 'repos')
export const worktreesDir = () => join(app.getPath('userData'), 'worktrees')

const bareRepoPath = (owner: string, repo: string) => join(reposDir(), owner, `${repo}.git`)

const git = (args: string[], cwd?: string) => run('git', args, { cwd, env: { ...process.env, GIT_TERMINAL_PROMPT: '0' } })

async function remoteUrl(owner: string, repo: string): Promise<string> {
  const protocol = (await run('gh', ['config', 'get', 'git_protocol']).catch(() => 'https')).trim()
  return protocol === 'ssh' ? `git@github.com:${owner}/${repo}.git` : `https://github.com/${owner}/${repo}.git`
}

/**
 * One shared bare clone per repo, plus one worktree per PR checked out at the
 * PR head. Re-opening a PR reuses its worktree, so build outputs and
 * language server caches survive between sessions.
 */
export async function checkout(pr: PRDetails, progress: (msg: string) => void): Promise<Checkout> {
  const bare = bareRepoPath(pr.owner, pr.repo)
  if (!existsSync(bare)) {
    progress(`Cloning ${pr.owner}/${pr.repo} (first time only)…`)
    await mkdir(dirname(bare), { recursive: true })
    await git(['clone', '--bare', '--no-tags', await remoteUrl(pr.owner, pr.repo), bare])
  }

  progress(`Fetching PR #${pr.number}…`)
  await git(
    ['fetch', '--no-tags', 'origin', `+refs/pull/${pr.number}/head:refs/review/pr/${pr.number}`, `+refs/heads/${pr.baseRef}:refs/review/base/${pr.baseRef}`],
    bare
  )

  const mergeBase = (await git(['merge-base', pr.baseSha, pr.headSha], bare)).trim()

  const root = join(worktreesDir(), pr.owner, pr.repo, `pr-${pr.number}`)
  progress('Checking out PR head…')
  if (existsSync(join(root, '.git'))) {
    await git(['checkout', '--detach', '--force', pr.headSha], root)
  } else {
    await git(['worktree', 'prune'], bare)
    await mkdir(dirname(root), { recursive: true })
    await git(['worktree', 'add', '--detach', '--force', root, pr.headSha], bare)
  }

  return { root, mergeBase, headSha: pr.headSha }
}

/** File contents at a commit, or null if the file doesn't exist there. */
export async function readAtCommit(owner: string, repo: string, sha: string, path: string): Promise<string | null> {
  if (!/^[0-9a-f]{7,64}$/.test(sha)) throw new Error(`Invalid commit: ${sha}`)
  try {
    return await git(['show', `${sha}:${path}`], bareRepoPath(owner, repo))
  } catch {
    return null
  }
}
