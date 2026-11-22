import {
  FileSystemProviderCapabilities,
  FileSystemProviderError,
  FileSystemProviderErrorCode,
  FileType,
  type IFileSystemProviderWithFileReadWriteCapability,
  type IStat
} from '@codingame/monaco-vscode-files-service-override'
import type * as vscode from 'vscode'

type Uri = vscode.Uri
const noEvent = (() => ({ dispose() {} })) as never

/**
 * Read-only `file://` provider backed by the main process, so editor models
 * for the worktree (and anything go-to-definition jumps to) come from disk.
 */
export class DiskFileSystemProvider implements IFileSystemProviderWithFileReadWriteCapability {
  readonly capabilities =
    FileSystemProviderCapabilities.FileReadWrite | FileSystemProviderCapabilities.PathCaseSensitive | FileSystemProviderCapabilities.Readonly
  readonly onDidChangeCapabilities = noEvent
  readonly onDidChangeFile = noEvent

  watch() {
    return { dispose() {} }
  }

  async stat(resource: Uri): Promise<IStat> {
    const s = await window.api.fsStat(resource.fsPath)
    if (!s) throw FileSystemProviderError.create('File not found', FileSystemProviderErrorCode.FileNotFound)
    return { type: s.type === 'directory' ? FileType.Directory : FileType.File, size: s.size, mtime: s.mtime, ctime: s.ctime }
  }

  async readFile(resource: Uri): Promise<Uint8Array> {
    try {
      return await window.api.fsRead(resource.fsPath)
    } catch {
      throw FileSystemProviderError.create('File not found', FileSystemProviderErrorCode.FileNotFound)
    }
  }

  async readdir(resource: Uri): Promise<[string, FileType][]> {
    const entries = await window.api.fsReaddir(resource.fsPath)
    return entries.map(([name, type]) => [name, type === 'directory' ? FileType.Directory : FileType.File])
  }

  private readonly denied = () => {
    throw FileSystemProviderError.create('Review checkouts are read-only', FileSystemProviderErrorCode.NoPermissions)
  }
  writeFile = async () => this.denied()
  mkdir = async () => this.denied()
  delete = async () => this.denied()
  rename = async () => this.denied()
}
