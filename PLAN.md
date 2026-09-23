# bro — full-Rust browser

Goal: browser that feels like Helium, runs faster than anything else. Shell first, engine second.

Current optimization results, new session features, validation, and known limitations: see [PERFORMANCE.md](PERFORMANCE.md).

## Reality check (one line)
A from-scratch engine (HTML/CSS/JS/JIT) that beats Blink+V8 on Speedometer/JetStream is a multi-year,
multi-person job. Only full-Rust engine that exists today is Servo. Plan embeds Servo for phase 2,
keeps a clean seam so a custom engine can replace it later.

## Phase 1 — Shell UI (now)
Stack: `iced 0.14` (wgpu backend, retained widget tree, themeable). Upgrade path if iced hits a
ceiling: raw `winit + wgpu + vello + parley` (no framework, max control).

Helium design tokens (from Helium's LayoutConstant overrides):
| token                 | px |
|-----------------------|----|
| base padding          | 3  |
| tab height            | 31 |
| toolbar button        | 28 |
| location bar height   | 28 |
| location bar margin   | 6  |
| tab / control radius  | 8  |
| location bar radius   | 6  |
| toolbar divider       | 1  |
| tabstrip overlap      | 0  |

Layout modes (Helium): Classic, Dynamic (tab strip hidden when 1 tab), Compact, Vertical.
Prototype ships Vertical only (horizontal strip removed). Compact/Zen next.

Deliverables:
- [x] cargo project at repo root
- [x] tabs: close, new-tab, active state
- [x] toolbar: back/forward/reload, omnibox, menu
- [x] content area: new-tab page placeholder (engine surface later)
- [x] dark + light theme
- [x] animations: hover fades, theme crossfade, popup fade/slide, sidebar slide, tab grow/select, content fade (tab close + omnibox focus still instant)
- [x] keyboard: Cmd+T / Cmd+W / Cmd+L / Cmd+R / Cmd+[ ] / Cmd+1..9
- [x] vertical tabs sidebar (only layout; ▤ hides/shows it)
- [ ] compact mode, zen mode (6px edge reveal, 200ms)
- [ ] window: frameless / custom title bar
- [ ] tab overflow (shrink, then scroll), tab hover state, favicons, drag-reorder
- [x] app menu (⋮): full Helium item set, zoom row, fullscreen
- [x] profiles: avatar button, per-profile tab sessions, add/switch
- [ ] internal pages (bro://settings, history, …) are placeholders
- [ ] submenus (History ›, Bookmarks › …) open a page instead of nesting
- [ ] New Window = new tab until iced::daemon multi-window
- [ ] settings page, bookmarks bar, svg icons (text glyphs now)

Run: `cargo run` · Test: `cargo test` · Window-only screenshot trick: `screencapture -l <CGWindowID>`

## Phase 2 — Engine
- [x] Servo embedded (`src/engine.rs`): per-tab `WebView` on a `SoftwareRenderingContext`, frame read back
      to RGBA → iced `image`. Mouse/scroll/keys forwarded. Servo boots lazily on first http(s) URL.
- [ ] GPU path: `WindowRenderingContext` / shared GL texture instead of CPU read-back (biggest perf lever)
- [ ] delegate: popups (`request_create_new`), permission prompts, auth, favicon, cursor shape, IME
- [ ] `bro://` internal pages served through `protocol_registry`
- Servo lives in `./servo` as our fork: a partial clone (`--filter=blob:none`, full history) on
  branch `bro`, forked from upstream `c6b9877a`. Edit it directly; `cargo build` at the root picks it
  up as a path dependency. Sync with upstream: `cd servo && git fetch origin && git rebase origin/main`.
  It is a nested git repo; turn it into a submodule once a GitHub fork exists.
- Frame path today: Servo paints ~display-rate for animated pages, each frame is a full CPU read-back
  (12 MB at 1918×1530) + GPU upload → ~25% of a core. Shell holds the previous `image::Allocation`
  until the next upload lands, which is what stopped the gray flicker. Next: own wgpu texture updated
  in place (`iced::widget::shader`), then zero-copy GL→wgpu.
- `bro <url>` opens straight into a page; `BRO_TRACE=1` prints frame plumbing; `RUST_LOG=warn` for Servo.
- Net stack: `hyper`/`reqwest` + `rustls`, HTTP/3 via `quinn`.
- Ad-block: `adblock` crate (Brave's engine, Rust) — Helium ships uBO; this is the Rust equivalent.

## Phase 3 — Perf
- Process model: per-site content processes (`ipc-channel`).
- Profile-guided: `cargo pgo`, `mimalloc`, `lto=fat`, `codegen-units=1` (already in release profile).
- Benchmarks tracked in CI: Speedometer 3, JetStream 2, MotionMark, cold start, memory/tab.
- Only after this: decide whether a custom engine is worth it. Servo's SpiderMonkey binding is the
  JS ceiling; beating V8 means own JIT. Not before phase 3 numbers exist.

## Layout
```
src/main.rs   shell: state, update, view, styles
PLAN.md       this
```
