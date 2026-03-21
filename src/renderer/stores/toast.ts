import { create } from 'zustand'

export type ToastType = 'success' | 'error' | 'info'

export interface Toast {
  id: string
  message: string
  type: ToastType
  duration: number
}

interface ToastState {
  toasts: Toast[]
  addToast: (message: string, type: ToastType, duration?: number) => void
  removeToast: (id: string) => void
}

export const useToastStore = create<ToastState>((set, get) => ({
  toasts: [],

  addToast: (message: string, type: ToastType, duration = 3000) => {
    const id = crypto.randomUUID()
    const toast: Toast = { id, message, type, duration }

    set((state) => ({ toasts: [...state.toasts, toast] }))

    // Auto-remove after duration
    setTimeout(() => {
      get().removeToast(id)
    }, duration)
  },

  removeToast: (id: string) => {
    set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) }))
  }
}))
