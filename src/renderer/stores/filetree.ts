import { create } from 'zustand'

interface FileTreeState {
  showFileTree: boolean
  searchQuery: string

  toggleFileTree: () => void
  setShowFileTree: (show: boolean) => void
  setSearchQuery: (query: string) => void
}

export const useFileTreeStore = create<FileTreeState>((set) => ({
  showFileTree: false,
  searchQuery: '',

  toggleFileTree: () => set((s) => ({ showFileTree: !s.showFileTree })),
  setShowFileTree: (show: boolean) => set({ showFileTree: show }),
  setSearchQuery: (query: string) => set({ searchQuery: query })
}))
