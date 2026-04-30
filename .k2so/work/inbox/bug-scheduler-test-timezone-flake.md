---
title: "Bug: scheduler::tests::hourly_outside_window_does_not_fire flakes by timezone"
priority: low
type: bug
created: 2026-04-30
---

## What

`crates/k2so-core/src/scheduler.rs::tests::hourly_outside_window_does_not_fire`
asserts that `should_project_fire_with_now` returns false for `now =
mk_now(2026, 4, 19, 18, 0)` against a `09:00-17:00` window. The
function is correct; the test fixture is wrong.

## Why it fails on some machines, passes on others

`mk_now` builds a `DateTime<Local>` by converting a UTC datetime
through `Local::from_utc_datetime`:

```rust
fn mk_now(year, month, day, h, m) -> DateTime<Local> {
    Local.from_utc_datetime(
        &Utc.with_ymd_and_hms(year, month, day, h, m, 0).unwrap().naive_utc(),
    )
}
```

So `mk_now(…, 18, 0)` doesn't mean "local 18:00" — it means "UTC 18:00
expressed as a local datetime." On a machine in MDT (UTC-6), that's
local 12:00, which IS inside the 09:00-17:00 window, so the function
correctly returns true and the test's `!should_fire` assertion
fails.

The fixture comment ("tests choose daylight hours to stay safe")
acknowledges the timezone fragility but assumes test times happen to
fall outside the schedule window in any reasonable zone — that
assumption is wrong for `mk_now(…, 18, 0)` against a 09:00-17:00
window for anywhere west of UTC-2.

Other tests using `mk_now` are passing because their (year, month,
day, h, m) inputs happen to land outside the asserted windows in
mountain/pacific zones too — but that's coincidence, not by design.

## Production impact

**None.** Production runtime correctly evaluates schedules against
the OS-supplied local time. "Fires every day at 7AM" means 7AM in
whatever timezone the laptop is currently set to. Last-fired
comparisons use timezone-aware RFC3339 stamps. A user moving from
Denver to NYC sees the heartbeat fire at the new local 7AM the next
morning, exactly as labeled.

This is a test-fixture bug, not a scheduler bug.

## Fix

One-line change in `mk_now` — build the `DateTime<Local>` directly
from local components instead of converting from UTC:

```rust
fn mk_now(year: i32, month: u32, day: u32, h: u32, m: u32) -> DateTime<Local> {
    Local.with_ymd_and_hms(year, month, day, h, m, 0).unwrap()
}
```

After the swap, every test that uses `mk_now(…, h, m)` reads the
hour/minute as local-time-of-day directly, removing the timezone
shift. Audit the existing tests' expected fire/no-fire outcomes to
make sure none of them implicitly depended on the UTC-shift before
flipping the helper — most should be fine since the comments already
intend "local time" semantics.

## Acceptance

- `cargo test -p k2so-core --lib scheduler::tests` passes on a
  laptop in any North American timezone (and ideally any timezone).
- No production behavior changes; this is purely a fixture cleanup.
