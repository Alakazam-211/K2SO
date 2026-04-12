---
title: "Bug: Shadow terminal reflow cols slightly too wide — entry line wraps on mobile"
priority: high
assigned_by: external
created: 2026-04-12
type: bug
source: manual
---

## Description

The shadow terminal reflow is wrapping text, but the calculated column count is slightly too wide for the actual visible area on the phone. This causes the CLI LLM tool's input/entry line (e.g., Claude Code's `> ` prompt line) to wrap onto a second line, which looks broken.

## Screenshot

See: `/Users/z3thon/Downloads/Screenshot 2026-04-12 at 10.54.50 AM.png`

The terminal content wraps mostly correctly, but notice the entry/prompt line at the bottom wraps — it's formatted for slightly more columns than the phone screen can display.

## Root Cause

The mobile app calculates columns as:
```
cols = Math.floor((containerWidth - 16px padding) / cellWidth)
```

Where `cellWidth` is measured by rendering a "W" character in the terminal font (SF Mono, 10px). This calculation may be off by 1-2 columns due to:

1. **Sub-pixel rounding** — `cellWidth` is a float (e.g., 6.0166px) but the reflow engine uses integer columns
2. **Font metric differences** — the mobile webview's font rendering may differ slightly from the server's calculation of what fits in N columns
3. **The CLI tool's prompt** includes special characters (cursor positioning, colors) that take visual space differently than plain text

## Suggested Fix

The reflow engine should subtract 1-2 columns as a safety margin when the dimensions come from a mobile client. Or the mobile app should send `cols - 1` to ensure nothing wraps unexpectedly.

Alternatively, the reflow engine could accept a `pixelWidth` parameter instead of `cols` and compute the column count server-side using the same font metrics.

## Mobile App Code Reference

The column calculation is in:
`/Users/z3thon/DevProjects/Alakazam Labs/K2SO-companion/src/components/TerminalView.tsx` line 153-158

```typescript
const calculateDims = useCallback(() => {
    const rect = containerRef.current.getBoundingClientRect();
    const cols = Math.max(10, Math.floor((rect.width - 16) / cellWRef.current));
    const rows = Math.max(5, Math.floor(rect.height / LINE_HEIGHT));
    return { cols, rows };
}, []);
```

## Quick Fix Option

We can subtract 2 columns on the mobile side as a safety margin:
```typescript
const cols = Math.max(10, Math.floor((rect.width - 16) / cellWRef.current) - 2);
```

But this is a band-aid. The real fix is aligning the font metrics between the mobile renderer and the reflow engine.
