import './languages'
import type { OpenEditor } from '@codingame/monaco-vscode-editor-service-override'
import { registerFileSystemOverlay } from '@codingame/monaco-vscode-files-service-override'
import { MonacoVscodeApiWrapper } from 'monaco-languageclient/vscodeApiWrapper'
import { useWorkerFactory, Worker } from 'monaco-languageclient/workerFactory'
import editorWorkerUrl from '@codingame/monaco-vscode-editor-api/esm/vs/editor/editor.worker.js?worker&url'
import textMateWorkerUrl from '@codingame/monaco-vscode-textmate-service-override/worker?worker&url'
import { DiskFileSystemProvider } from './diskfs'

/**
 * Called when VS Code wants to show a different file (go to definition,
 * references, etc). The EditorHost installs the real handler.
 */
let openEditorHandler: OpenEditor = async () => undefined
export function setOpenEditorHandler(handler: OpenEditor): void {
  openEditorHandler = handler
}

let started: Promise<void> | undefined

/** Initializes VS Code's services once per app lifetime. */
export function initServices(): Promise<void> {
  started ??= (async () => {
    const wrapper = new MonacoVscodeApiWrapper({
      $type: 'extended',
      viewsConfig: {
        $type: 'EditorService',
        openEditorFunc: (modelRef, options, sideBySide) => openEditorHandler(modelRef, options, sideBySide)
      },
      userConfiguration: {
        json: JSON.stringify({
          'workbench.colorTheme': 'Default Dark Modern',
          'editor.fontSize': 13,
          'editor.minimap.enabled': false,
          'editor.semanticHighlighting.enabled': true,
          'editor.wordBasedSuggestions': 'off',
          'editor.hover.delay': 250,
          'editor.gotoLocation.multipleDefinitions': 'peek',
          'editor.gotoLocation.multipleReferences': 'peek',
          'diffEditor.renderSideBySide': true,
          'diffEditor.hideUnchangedRegions.enabled': false,
          'files.associations': { '*.h': 'cpp' }
        })
      },
      monacoWorkerFactory: (logger) =>
        useWorkerFactory({
          logger,
          workerLoaders: {
            editorWorkerService: () => new Worker(new URL(editorWorkerUrl, location.href), { type: 'module' }),
            TextMateWorker: () => new Worker(new URL(textMateWorkerUrl, location.href), { type: 'module' })
          }
        })
    })
    await wrapper.start()
    registerFileSystemOverlay(1, new DiskFileSystemProvider())
  })()
  return started
}

