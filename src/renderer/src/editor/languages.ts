// Grammars and language definitions, registered as VS Code extensions.
import '@codingame/monaco-vscode-go-default-extension'
import '@codingame/monaco-vscode-cpp-default-extension'
import '@codingame/monaco-vscode-python-default-extension'
import { registerExtension, type RegisterLocalExtensionResult } from '@codingame/monaco-vscode-api/extensions'
import pythonGrammarUrl from '@codingame/monaco-vscode-python-default-extension/resources/MagicPython.tmLanguage.json?url'
import pythonLanguageConfigUrl from '@codingame/monaco-vscode-python-default-extension/resources/language-configuration.json?url'

/**
 * Starlark (Bazel BUILD/.bzl files). VS Code has no built-in grammar for it;
 * Starlark is a Python dialect, so it reuses the Python grammar — the same
 * approach VS Code's Bazel extension started from.
 */
const { registerFileUrl } = registerExtension(
  {
    name: 'starlark',
    publisher: 'review-tool',
    version: '1.0.0',
    engines: { vscode: '*' },
    contributes: {
      languages: [
        {
          id: 'starlark',
          aliases: ['Starlark', 'Bazel'],
          extensions: ['.bzl', '.star', '.bazel', '.sky'],
          filenames: ['BUILD', 'WORKSPACE', 'WORKSPACE.bzlmod', 'MODULE.bazel'],
          configuration: './language-configuration.json'
        }
      ],
      // Same scope as Python: the grammar file declares `source.python`, and TextMate looks grammars up by that name.
      grammars: [{ language: 'starlark', scopeName: 'source.python', path: './syntaxes/python.tmLanguage.json' }]
    }
  },
  undefined,
  { system: true }
  // Same call shape the generated @codingame extension packages use; it does return registerFileUrl.
) as RegisterLocalExtensionResult
registerFileUrl('language-configuration.json', new URL(pythonLanguageConfigUrl, location.href).toString())
registerFileUrl('syntaxes/python.tmLanguage.json', new URL(pythonGrammarUrl, location.href).toString())
