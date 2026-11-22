import { execFile } from 'node:child_process'
import { homedir } from 'node:os'

/**
 * Apps launched from Finder don't inherit the shell PATH, so add the usual
 * locations for gh, git, go, gopls and clangd.
 */
export function augmentPath(): void {
  const extra = [`${homedir()}/go/bin`, '/opt/homebrew/bin', '/usr/local/bin', '/usr/bin']
  const current = (process.env.PATH ?? '').split(':').filter(Boolean)
  process.env.PATH = [...new Set([...current, ...extra])].join(':')
}

export function run(cmd: string, args: string[], opts: { cwd?: string; env?: NodeJS.ProcessEnv } = {}): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(cmd, args, { ...opts, maxBuffer: 512 * 1024 * 1024 }, (err, stdout, stderr) => {
      if (err) {
        reject(new Error(`${cmd} ${args.join(' ')} failed: ${stderr.trim() || err.message}`))
      } else {
        resolve(stdout)
      }
    })
  })
}
