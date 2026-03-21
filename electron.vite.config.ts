import { defineConfig, externalizeDepsPlugin } from 'electron-vite'
import tailwindcss from '@tailwindcss/vite'
import { resolve } from 'path'
import { existsSync } from 'fs'
import type { Plugin } from 'vite'

/**
 * Resolve @/ and @shared/ path aliases for the renderer build.
 * Uses a resolveId hook since electron-vite processes resolve.alias
 * through its own config pipeline.
 */
function pathAliasPlugin(): Plugin {
  return {
    name: 'k2so-path-aliases',
    enforce: 'pre',
    resolveId(source: string) {
      const cwd = process.cwd()
      let basePath: string | null = null

      if (source.startsWith('@shared/')) {
        basePath = resolve(cwd, 'src/shared', source.slice(8))
      } else if (source.startsWith('@/')) {
        basePath = resolve(cwd, 'src/renderer', source.slice(2))
      }

      if (!basePath) return null

      // Try with extensions
      for (const ext of ['.ts', '.tsx', '.js', '.jsx', '.json', '']) {
        if (existsSync(basePath + ext)) return basePath + ext
      }
      // Try index files
      for (const ext of ['.ts', '.tsx', '.js', '.jsx']) {
        const indexPath = resolve(basePath, 'index' + ext)
        if (existsSync(indexPath)) return indexPath
      }

      return null
    }
  }
}

export default defineConfig({
  main: {
    plugins: [externalizeDepsPlugin({ exclude: ['superjson'] })],
    build: {
      rollupOptions: {
        external: ['better-sqlite3', 'node-pty']
      }
    }
  },
  preload: {
    plugins: [externalizeDepsPlugin()]
  },
  renderer: {
    plugins: [tailwindcss(), pathAliasPlugin()]
  }
})
