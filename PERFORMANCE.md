# Browser and Servo improvements

## What changed

- Only the selected tab is shown, focused, painted, and read back to the shell. Background tabs retain their latest dirty state and cached frame; resizing them is deferred until selection. This does not suspend their JavaScript, networking, or media.
- Servo wake requests coalesce behind one pending notification. Acknowledgment happens before processing the wake so requests arriving during processing are retained.
- Each WebView has at most one image upload in flight. Dirty notifications survive until its upload completes. Completion includes the WebView ID, preventing an old upload from attaching to a replacement WebView with the same shell tab ID.
- Servo flips framebuffer rows in place instead of cloning the entire RGBA image.
- Search queries use URL encoding. Localhost and loopback addresses accept development-server ports. Pixel-converted wheel deltas now report pixel units.
- Key repeats reach web pages. Switching tabs or moving focus to browser controls releases held page keys. Clicking page content and submitting a URL release omnibox focus.
- Internal navigation closes the old page view instead of continuing to show and run it. Browser load and crash notifications now appear in the UI.
- Closed tabs can be reopened with Cmd/Ctrl+Shift+T (last 20 per profile). Restored views receive fresh IDs, their previous zoom, and retained URL history. Live Servo history notifications keep the saved history current. Cmd/Ctrl+9 selects the last tab.
- Cmd/Ctrl+D or the toolbar star toggles a session bookmark; the Bookmarks menu opens the list. Theme and sidebar settings are functional.
- The private-window command now explains that isolated storage is unavailable. It no longer creates a normal shared-storage tab labeled as incognito.

## Reproducible microbenchmark

```sh
rustc -O --edition=2024 tools/readback-bench.rs -o /tmp/bro-readback-bench
/tmp/bro-readback-bench
rustc --edition=2024 --test servo/components/shared/paint/rendering_context/rows.rs -o /tmp/bro-row-tests
/tmp/bro-row-tests
cargo check --locked
cargo test --locked
```

One local run on September 22, 2026 (200 iterations, optimized Rust):

| RGBA frame | Previous clone + copy | In-place swap | Speedup | Temporary allocation removed |
| --- | ---: | ---: | ---: | ---: |
| 1918 × 1530 | 1.495 ms | 0.598 ms | 2.50× | 11,738,160 bytes/frame |
| 3840 × 2160 | 3.156 ms | 1.839 ms | 1.72× | 33,177,600 bytes/frame |

This measures the row-flip operation only, under local build activity. It excludes page layout, GL readback, GPU upload, and presentation. It is not a browser-wide speedup or a Speedometer result. The test compares byte-for-byte output against the old copy algorithm across empty, small, odd-height, and even-height buffers.

## Remaining limitations

The renderer still copies frames from Servo to CPU memory and uploads them into iced. Shared textures or an integrated compositor remain the largest architectural opportunity. Background tabs still allocate rendering contexts and cached frames, and scripts continue running. End-to-end CPU, frame latency, and memory benchmarks are still needed.

Profiles separate shell tab sessions, not cookies or origin storage. Bookmarks and closed tabs are in memory only. Private browsing needs isolated storage contexts. Several existing menu items remain placeholders; popups, permissions, IME, downloads, and multi-window support need dedicated implementations. Restored history navigates by URL; it does not restore JavaScript state or scroll positions, and redirects may change the restored forward history.

Servo is a nested repository currently untracked by the outer repository. The paint change and new `components/shared/paint/rendering_context/rows.rs` must be preserved in that repository when committing or distributing this project. Existing media-source changes were left intact.

## Validation

- `cargo check --locked`: passed.
- `cargo build --locked`: passed.
- `cargo test --locked`: 15 passed.
- Standalone framebuffer row tests: 1 passed.
- Scoped formatting checks and `git diff --check`: passed. The full vendored Servo tree has pre-existing formatting differences.
- Launched the built executable against a local HTML page with CSS animation. Trace logs confirmed repeated successful frame readbacks and GPU allocations; the test instance was then stopped. Full interactive UI coverage was not automated because the computer-use app inventory did not expose the unbundled executable.
- Existing Servo dependency warnings remain.
