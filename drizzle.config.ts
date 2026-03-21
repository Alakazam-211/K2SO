import { defineConfig } from 'drizzle-kit'
import { resolve } from 'path'

export default defineConfig({
  schema: './src/main/lib/db/schema.ts',
  out: './drizzle',
  dialect: 'sqlite'
})
