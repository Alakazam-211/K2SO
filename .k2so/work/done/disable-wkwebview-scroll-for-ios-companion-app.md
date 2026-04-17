---
title: Disable WKWebView scroll for iOS companion app
priority: high
assigned_by: user
created: 2026-04-13
type: task
source: manual
---

Tauri iOS: Disable WKWebView scroll view for companion app

The K2 companion app has a standard chat layout (fixed header + scrollable content + fixed input bar).
On iOS, when the keyboard opens, WKWebView scrolls its own scroll view, pushing the header off screen.
No CSS or JS workaround can fix this because the scroll happens at the native UIScrollView level.

The fix is to disable WKWebView's scroll view from the native side:

    webView.scrollView.isScrollEnabled = false
    webView.scrollView.contentInsetAdjustmentBehavior = .never

This is documented in these Tauri issues:
- https://github.com/tauri-apps/tauri/discussions/9368
- https://github.com/tauri-apps/tauri/issues/9907
- https://github.com/tauri-apps/tauri/issues/10631

Options:
1. Add Swift code to the iOS project that hooks into the WKWebView after launch
2. Use objc2 crates from Rust to access the scroll view
3. Request upstream Tauri support via issue 13200

The companion app already has tauri-plugin-ios-keyboard which provides keyboard
height events. Combined with scroll view disabled, the app can handle layout
entirely through CSS flexbox.
