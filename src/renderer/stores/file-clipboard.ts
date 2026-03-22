import { create } from 'zustand'

export type ClipboardMode = 'copy' | 'cut'

interface FileClipboardState {
  paths: string[]
  mode: ClipboardMode | null

  copy: (paths: string[]) => void
  cut: (paths: string[]) => void
  clear: () => void
  hasPaths: () => boolean
}

export const useFileClipboardStore = create<FileClipboardState>((set, get) => ({
  paths: [],
  mode: null,

  copy: (paths: string[]) => {
    set({ paths, mode: 'copy' })
  },

  cut: (paths: string[]) => {
    set({ paths, mode: 'cut' })
  },

  clear: () => {
    set({ paths: [], mode: null })
  },

  hasPaths: () => {
    return get().paths.length > 0
  }
}))
