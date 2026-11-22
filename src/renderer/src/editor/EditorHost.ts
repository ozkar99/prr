import { createModelReference, type ITextFileEditorModel } from '@codingame/monaco-vscode-api/monaco'
import * as monaco from '@codingame/monaco-vscode-editor-api'
import type { IReference } from '@codingame/monaco-vscode-editor-service-override'
import * as vscode from 'vscode'
import type { Checkout, PRDetails, PRFile } from '../../../shared/types'
import type { CommentController } from '../review/CommentController'
import { setOpenEditorHandler } from './services'

type Position = { lineNumber: number; column: number }

export type Location =
  | { kind: 'diff'; file: PRFile; position?: Position }
  | { kind: 'file'; path: string; position?: Position }

type Held = { dispose(): void }

let baseModelSeq = 0

/**
 * Owns the center pane. Shows either a side-by-side diff (for files changed
 * in the PR) or a plain read-only editor (for anything navigated to), and
 * keeps a back stack like an IDE.
 */
export class EditorHost {
  private diffEditor?: monaco.editor.IStandaloneDiffEditor
  private codeEditor?: monaco.editor.IStandaloneCodeEditor
  private held: Held[] = []
  private backStack: Location[] = []
  private current?: Location
  private filesByPath: Map<string, PRFile>

  constructor(
    private readonly container: HTMLElement,
    private readonly pr: PRDetails,
    private readonly checkout: Checkout,
    private readonly onLocationChange: (loc: Location | undefined, canGoBack: boolean) => void,
    private readonly comments?: CommentController
  ) {
    this.filesByPath = new Map(pr.files.map((f) => [f.path, f]))
    setOpenEditorHandler(async (modelRef, options) => {
      const path = modelRef.object.textEditorModel.uri.fsPath
      const sel = (options as { selection?: { startLineNumber: number; startColumn: number } } | undefined)?.selection
      const position = sel ? { lineNumber: sel.startLineNumber, column: sel.startColumn } : undefined
      const file = this.prFileForPath(path)

      // Target is in the file already on screen: just move the cursor.
      const onScreen = this.current?.kind === 'diff' ? this.current.file.path === file?.path : this.current?.path === path
      if (onScreen && this.activeEditor) {
        modelRef.dispose()
        this.pushCurrent()
        if (position) {
          this.activeEditor.setPosition(position)
          this.activeEditor.revealPositionInCenter(position)
        }
        this.onLocationChange(this.current, true)
        return this.activeEditor
      }

      this.pushCurrent()
      if (file && file.status !== 'removed') {
        modelRef.dispose()
        await this.showDiff(file, position)
        return this.diffEditor!.getModifiedEditor()
      }
      this.showFile(path, modelRef as IReference<ITextFileEditorModel>, position)
      return this.codeEditor!
    })
  }

  /** The editor showing the PR head (modified side of a diff, or the plain file editor). */
  get activeEditor(): monaco.editor.ICodeEditor | undefined {
    return this.diffEditor?.getModifiedEditor() ?? this.codeEditor
  }

  get location(): Location | undefined {
    return this.current
  }

  /** Path relative to the worktree, or undefined if outside it. */
  relativePath(absPath: string): string | undefined {
    const prefix = this.checkout.root + '/'
    return absPath.startsWith(prefix) ? absPath.slice(prefix.length) : undefined
  }

  private prFileForPath(absPath: string): PRFile | undefined {
    const rel = this.relativePath(absPath)
    return rel === undefined ? undefined : this.filesByPath.get(rel)
  }

  async openDiff(file: PRFile): Promise<void> {
    this.pushCurrent()
    await this.showDiff(file)
  }

  async openFile(absPath: string, position?: Position): Promise<void> {
    this.pushCurrent()
    const ref = await createModelReference(vscode.Uri.file(absPath))
    this.showFile(absPath, ref, position)
  }

  async back(): Promise<void> {
    const loc = this.backStack.pop()
    if (!loc) return
    if (loc.kind === 'diff') {
      await this.showDiff(loc.file, loc.position)
    } else {
      const ref = await createModelReference(vscode.Uri.file(loc.path))
      this.showFile(loc.path, ref, loc.position)
    }
  }

  dispose(): void {
    setOpenEditorHandler(async () => undefined)
    this.comments?.detach()
    this.releaseModels()
    this.diffEditor?.dispose()
    this.codeEditor?.dispose()
  }

  private pushCurrent(): void {
    if (!this.current) return
    const editor = this.current.kind === 'diff' ? this.diffEditor?.getModifiedEditor() : this.codeEditor
    const position = editor?.getPosition() ?? undefined
    this.backStack.push({ ...this.current, position })
    if (this.backStack.length > 100) this.backStack.shift()
  }

  private releaseModels(): void {
    for (const h of this.held) h.dispose()
    this.held = []
  }

  private setLocation(loc: Location): void {
    this.current = loc
    this.onLocationChange(loc, this.backStack.length > 0)
  }

  private async showDiff(file: PRFile, position?: Position): Promise<void> {
    const { owner, repo } = this.pr
    const absPath = `${this.checkout.root}/${file.path}`

    // Load the new models before releasing the old ones so the editor never flashes empty.
    const next: Held[] = []
    let modified: monaco.editor.ITextModel
    if (file.status === 'removed') {
      modified = monaco.editor.createModel('', undefined, vscode.Uri.from({ scheme: 'pr-removed', path: '/' + file.path, query: String(++baseModelSeq) }))
      next.push(modified)
    } else {
      const ref = await createModelReference(vscode.Uri.file(absPath))
      next.push(ref)
      modified = ref.object.textEditorModel!
    }

    const baseText =
      file.status === 'added' ? '' : ((await window.api.readBase(owner, repo, this.checkout.mergeBase, file.previousPath ?? file.path)) ?? '')
    const original = monaco.editor.createModel(
      baseText,
      modified.getLanguageId(),
      // Unique per load: the previous model for this path is still alive until the swap below.
      vscode.Uri.from({ scheme: 'pr-base', path: '/' + (file.previousPath ?? file.path), query: `${this.checkout.mergeBase}.${++baseModelSeq}` })
    )
    if (file.status === 'removed') {
      monaco.editor.setModelLanguage(modified, original.getLanguageId())
    }
    next.push(original)

    this.codeEditor?.dispose()
    this.codeEditor = undefined
    this.comments?.detach()
    if (!this.diffEditor) {
      this.diffEditor = monaco.editor.createDiffEditor(this.container, {
        readOnly: true,
        originalEditable: false,
        automaticLayout: true,
        renderSideBySide: true,
        ignoreTrimWhitespace: false,
        glyphMargin: true,
        renderMarginRevertIcon: false,
        hideUnchangedRegions: { enabled: false }
      })
    }
    this.diffEditor.setModel({ original, modified })
    this.releaseModels()
    this.held = next
    this.comments?.attach(this.diffEditor, file)

    const target = this.diffEditor.getModifiedEditor()
    if (position) {
      target.setPosition(position)
      target.revealPositionInCenter(position)
    } else {
      // The whole file is shown, so start at the first change once the diff is computed.
      const diff = this.diffEditor
      const sub = diff.onDidUpdateDiff(() => {
        sub.dispose()
        if (this.diffEditor !== diff || diff.getModel()?.modified !== modified) return
        const first = diff.getLineChanges()?.[0]
        if (!first) return
        // A pure deletion has modifiedEndLineNumber 0; its anchor is modifiedStartLineNumber.
        const line = Math.max(1, first.modifiedStartLineNumber)
        target.setPosition({ lineNumber: line, column: 1 })
        // The diff editor adds its alignment spacers right after this event, which moves the scroll; reveal after that.
        requestAnimationFrame(() => requestAnimationFrame(() => target.revealLineInCenter(line)))
      })
    }
    target.focus()
    this.setLocation({ kind: 'diff', file })
  }

  private showFile(absPath: string, ref: IReference<ITextFileEditorModel>, position?: Position): void {
    this.comments?.detach()
    this.diffEditor?.dispose()
    this.diffEditor = undefined
    if (!this.codeEditor) {
      this.codeEditor = monaco.editor.create(this.container, { readOnly: true, automaticLayout: true })
    }
    this.codeEditor.setModel(ref.object.textEditorModel!)
    this.releaseModels()
    this.held = [ref]
    if (position) {
      this.codeEditor.setPosition(position)
      this.codeEditor.revealPositionInCenter(position)
    }
    this.codeEditor.focus()
    this.setLocation({ kind: 'file', path: absPath })
  }
}
