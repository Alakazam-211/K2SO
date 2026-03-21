/**
 * Simple logger for the main process.
 * Wraps console methods so we can swap in file logging later.
 */
export const log = {
  info: (...args: unknown[]): void => {
    console.log(...args)
  },
  warn: (...args: unknown[]): void => {
    console.warn(...args)
  },
  error: (...args: unknown[]): void => {
    console.error(...args)
  }
}
