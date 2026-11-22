import { createRoot } from 'react-dom/client'
import { App } from './App'
import { initServices } from './editor/services'
import './styles.css'

const root = createRoot(document.getElementById('root')!)

// VS Code's services must be up before any editor is created.
initServices().then(
  () => root.render(<App />),
  (e: Error) => root.render(<pre className="error">Failed to start editor services: {e.stack ?? e.message}</pre>)
)
