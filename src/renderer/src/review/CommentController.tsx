import * as monaco from '@codingame/monaco-vscode-editor-api'
import { createRoot, type Root } from 'react-dom/client'
import type { DiffSide, PRFile } from '../../../shared/types'
import type { ReviewStore } from './ReviewStore'
import { ThreadZone, type ComposerRequest } from './ThreadZone'

/**
 * A thread area, built like VS Code's ZoneWidget: an empty view zone reserves
 * space between lines, and an overlay widget (above the text layer, so it can
 * take clicks and keys) holds the content and follows the zone's position.
 */
type Zone = {
  id: string
  zone: monaco.editor.IViewZone
  widget: monaco.editor.IOverlayWidget
  root: Root
  observer: ResizeObserver
}

let widgetSeq = 0

/** Lines GitHub accepts comments on; without patch info, allow any line and let the API decide. */
function isCommentable(file: PRFile, side: DiffSide, line: number): boolean {
  if (!file.commentable) return true
  return file.commentable[side].some(([a, b]) => line >= a && line <= b)
}

/** Whether a multi-line range stays inside a single hunk, as GitHub requires. */
function rangeCommentable(file: PRFile, side: DiffSide, start: number, end: number): boolean {
  if (!file.commentable) return true
  return file.commentable[side].some(([a, b]) => start >= a && end <= b)
}

const MT = monaco.editor.MouseTargetType

/**
 * Renders review threads for one side of the diff as view zones (React roots
 * inside the editor) and handles the gutter "+" and the add-comment action.
 */
class SideController {
  private zones = new Map<number, Zone>()
  private composers = new Map<number, ComposerRequest>()
  private hover: monaco.editor.IEditorDecorationsCollection
  private disposables: monaco.IDisposable[] = []

  constructor(
    private readonly editor: monaco.editor.IStandaloneCodeEditor,
    private readonly side: DiffSide,
    private readonly file: PRFile,
    private readonly store: ReviewStore
  ) {
    this.hover = editor.createDecorationsCollection()

    this.disposables.push(
      editor.onMouseMove((e) => {
        const line = e.target.position?.lineNumber
        const inLine = [MT.GUTTER_GLYPH_MARGIN, MT.GUTTER_LINE_NUMBERS, MT.GUTTER_LINE_DECORATIONS, MT.CONTENT_TEXT, MT.CONTENT_EMPTY].includes(e.target.type)
        if (line && inLine && isCommentable(file, side, line)) {
          this.hover.set([{ range: new monaco.Range(line, 1, line, 1), options: { glyphMarginClassName: 'add-comment-glyph', glyphMarginHoverMessage: { value: 'Add a comment' } } }])
        } else {
          this.hover.clear()
        }
      }),
      editor.onMouseLeave(() => this.hover.clear()),
      editor.onMouseDown((e) => {
        const line = e.target.position?.lineNumber
        if (e.target.type === MT.GUTTER_GLYPH_MARGIN && line && isCommentable(file, side, line)) {
          this.openComposer(line)
        }
      }),
      editor.addAction({
        id: `review.addComment.${side}`,
        label: 'Add Review Comment',
        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyMod.Alt | monaco.KeyCode.KeyC],
        contextMenuGroupId: 'navigation',
        contextMenuOrder: 0,
        run: (ed) => {
          const pos = ed.getPosition()
          if (pos) this.openComposer(pos.lineNumber)
        }
      }),
      editor.onDidLayoutChange(() => {
        for (const z of this.zones.values()) this.fitWidth(z.widget.getDomNode())
      }),
      { dispose: store.subscribe(() => this.sync()) }
    )
    this.sync()
  }

  /** Comments on the selection if it covers `line`, otherwise on `line` alone. */
  openComposer(line: number): void {
    if (!isCommentable(this.file, this.side, line)) return
    let request: ComposerRequest = {}
    const sel = this.editor.getSelection()
    if (sel) {
      // A selection ending at column 1 doesn't really include its last line.
      const start = sel.startLineNumber
      const end = sel.endColumn === 1 && sel.endLineNumber > start ? sel.endLineNumber - 1 : sel.endLineNumber
      if (end > start && line >= start && line <= end && rangeCommentable(this.file, this.side, start, end)) {
        line = end
        request = { startLine: start }
      }
    }
    this.composers.set(line, request)
    this.sync()
    this.editor.revealLineInCenterIfOutsideViewport(line)
  }

  private closeComposer(line: number): void {
    this.composers.delete(line)
    this.sync()
  }

  /** Adds/removes zones so there is exactly one per line with threads or an open composer. */
  private sync(): void {
    const state = this.store.get().state
    const lines = new Set<number>(this.composers.keys())
    for (const t of state?.threads ?? []) {
      if (t.path === this.file.path && t.side === this.side && t.line !== null) lines.add(t.line)
    }

    this.editor.changeViewZones((accessor) => {
      for (const [line, z] of this.zones) {
        if (!lines.has(line)) {
          accessor.removeZone(z.id)
          this.destroy(z)
          this.zones.delete(line)
        }
      }
      for (const line of lines) {
        if (!this.zones.has(line)) this.zones.set(line, this.createZone(accessor, line))
      }
    })
    for (const [line, z] of this.zones) this.render(line, z)
  }

  private createZone(accessor: monaco.editor.IViewZoneChangeAccessor, line: number): Zone {
    const overlay = document.createElement('div')
    overlay.className = 'thread-zone'
    overlay.style.top = '-100000px' // until Monaco reports the zone's position
    this.fitWidth(overlay)
    // Keep the editor's own handlers (selection, keybindings) out of the thread UI.
    for (const type of ['pointerdown', 'pointerup', 'mousedown', 'mouseup', 'click', 'dblclick', 'keydown', 'keyup', 'keypress', 'contextmenu', 'paste', 'copy', 'cut']) {
      overlay.addEventListener(type, (e) => e.stopPropagation())
    }
    const widget: monaco.editor.IOverlayWidget = {
      getId: () => `review.thread.${++widgetSeq}`,
      getDomNode: () => overlay,
      getPosition: () => null // positioned by hand from the zone's top
    }
    const widgetId = widget.getId()
    widget.getId = () => widgetId
    this.editor.addOverlayWidget(widget)

    const zone: monaco.editor.IViewZone = {
      afterLineNumber: line,
      heightInPx: 60,
      domNode: document.createElement('div'),
      // Monaco reports the zone's top in viewport coordinates, or far off-screen when scrolled out.
      onDomNodeTop: (top) => {
        overlay.style.top = `${top}px`
        // Overlay widgets draw above the sticky-scroll header; hide the part that slides under it.
        const sticky = this.editor.getDomNode()?.querySelector<HTMLElement>('.sticky-widget')
        const hidden = (sticky?.offsetHeight ?? 0) - top
        overlay.style.clipPath = hidden > 0 ? `inset(${hidden}px 0 0 0)` : ''
      }
    }
    const id = accessor.addZone(zone)
    const observer = new ResizeObserver(() => {
      const height = overlay.offsetHeight
      if (height && Math.abs(height - (zone.heightInPx ?? 0)) > 1) {
        zone.heightInPx = height
        this.editor.changeViewZones((a) => a.layoutZone(id))
      }
    })
    observer.observe(overlay)
    return { id, zone, widget, root: createRoot(overlay), observer }
  }

  /** Overlay widgets aren't laid out by Monaco when unpositioned; place it over the text area. */
  private fitWidth(overlay: HTMLElement): void {
    const layout = this.editor.getLayoutInfo()
    overlay.style.position = 'absolute'
    overlay.style.left = `${layout.contentLeft}px`
    overlay.style.width = `${Math.max(200, layout.contentWidth - layout.verticalScrollbarWidth - layout.minimap.minimapWidth)}px`
  }

  private render(line: number, z: Zone): void {
    z.root.render(
      <ThreadZone
        store={this.store}
        path={this.file.path}
        side={this.side}
        line={line}
        composer={this.composers.get(line)}
        onCloseComposer={() => this.closeComposer(line)}
      />
    )
  }

  private destroy(z: Zone): void {
    z.observer.disconnect()
    this.editor.removeOverlayWidget(z.widget)
    // Unmount after the current render pass; React warns on synchronous unmount during render.
    queueMicrotask(() => z.root.unmount())
  }

  dispose(): void {
    for (const d of this.disposables) d.dispose()
    this.hover.clear()
    this.editor.changeViewZones((accessor) => {
      for (const z of this.zones.values()) {
        accessor.removeZone(z.id)
        this.destroy(z)
      }
    })
    this.zones.clear()
  }
}

export class CommentController {
  private sides: SideController[] = []

  constructor(private readonly store: ReviewStore) {}

  /** Called whenever the diff editor shows a (new) file. */
  attach(diff: monaco.editor.IStandaloneDiffEditor, file: PRFile): void {
    this.detach()
    this.sides = [
      new SideController(diff.getOriginalEditor(), 'LEFT', file, this.store),
      new SideController(diff.getModifiedEditor(), 'RIGHT', file, this.store)
    ]
  }

  detach(): void {
    for (const s of this.sides) s.dispose()
    this.sides = []
  }
}
