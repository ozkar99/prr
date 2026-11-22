import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// Frontend only; Tauri (src-tauri/) is the backend.
export default defineConfig({
  root: 'src/renderer',
  plugins: [react()],
  worker: { format: 'es' },
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: 'esnext', outDir: '../../out/renderer', emptyOutDir: true }
})
