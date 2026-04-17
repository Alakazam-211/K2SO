---
title: Improve companion API docs: full WS param coverage
priority: normal
assigned_by: user
created: 2026-04-13
type: task
source: manual
---

Companion API documentation needs full parameter coverage for WS methods

The current API reference lists WS methods with incomplete params.
Example: terminal.read is listed as { project, id, lines? } but actually
supports scrollback as well. This caused us to assume WS terminal.read
could not do scrollback, wasting significant development time trying
alternative approaches.

Request:
- Document all supported params for every WS method (matching HTTP parity)
- Host the API reference in a persistent location (suggestion: docs/ in the
  K2SO repo, or a dedicated page on the k2so-website)
- Include example request/response JSON for each method
- Note which HTTP params are also available over WS

The current reference is an inbox notice file which gets buried.
A versioned doc in the repo or website would be more discoverable.
