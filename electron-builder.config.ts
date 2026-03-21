import type { Configuration } from 'electron-builder'

const config: Configuration = {
  appId: 'com.alakazamlabs.k2so',
  productName: 'K2SO',
  copyright: 'Copyright © 2024 Alakazam Labs',

  // ── Directories ─────────────────────────────────────────────────────
  directories: {
    buildResources: 'resources',
    output: 'dist'
  },

  // ── Files ───────────────────────────────────────────────────────────
  files: [
    'out/**/*',
    'package.json'
  ],

  // Include drizzle migrations in app resources
  extraResources: [
    {
      from: 'drizzle/',
      to: 'drizzle/',
      filter: ['**/*']
    }
  ],

  // ── ASAR ────────────────────────────────────────────────────────────
  asar: true,
  asarUnpack: [
    // Native modules must be unpacked from asar
    'node_modules/better-sqlite3/**',
    'node_modules/node-pty/**'
  ],

  // ── Native dependency rebuild ───────────────────────────────────────
  npmRebuild: true,
  nodeGypRebuild: false,
  buildDependenciesFromSource: true,

  // ── macOS ───────────────────────────────────────────────────────────
  mac: {
    target: [
      {
        target: 'dmg',
        arch: ['arm64', 'x64']
      }
    ],
    category: 'public.app-category.developer-tools',
    icon: 'resources/icon.png',
    darkModeSupport: true,
    hardenedRuntime: true,
    gatekeeperAssess: false,
    entitlements: 'build/entitlements.mac.plist',
    entitlementsInherit: 'build/entitlements.mac.plist',
    // Register k2so:// protocol
    extendInfo: {
      CFBundleURLTypes: [
        {
          CFBundleURLName: 'K2SO URL',
          CFBundleURLSchemes: ['k2so']
        }
      ]
    }
  },

  // ── DMG settings ────────────────────────────────────────────────────
  dmg: {
    window: {
      width: 540,
      height: 380
    },
    contents: [
      {
        x: 140,
        y: 180,
        type: 'file'
      },
      {
        x: 400,
        y: 180,
        type: 'link',
        path: '/Applications'
      }
    ],
    title: 'Install K2SO'
  },

  // ── Auto-update (publish) ───────────────────────────────────────────
  publish: {
    provider: 'github',
    owner: 'AlakazamLabs',
    repo: 'K2SO',
    releaseType: 'release'
  },

  // ── After sign (placeholder for notarization) ──────────────────────
  afterSign: undefined
}

export default config
