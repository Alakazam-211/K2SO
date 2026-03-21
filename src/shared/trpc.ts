/**
 * Re-export AppRouter type for renderer-side inference.
 *
 * This file bridges the main→renderer type boundary. The actual router
 * lives in src/main/lib/trpc/router.ts and is only imported as a type,
 * so no Node.js code leaks into the renderer bundle.
 */
export type { AppRouter } from '../main/lib/trpc/router'
