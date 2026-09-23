//! Servo glue. One `WebView` per web tab, software-rendered into an RGBA buffer that the shell
//! shows through an iced `image`. Input is forwarded from the shell's widgets.
//!
//! ponytail: CPU read-back of every frame (`read_to_image`). Upgrade path is a `WindowRenderingContext`
//! / shared GL texture blitted straight into iced's wgpu surface.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use dpi::PhysicalSize;
use euclid::Scale;
use iced::futures::channel::mpsc;
use iced::widget::image;
use iced::{Subscription, keyboard, mouse};
use servo::{
    Code, DeviceIntRect, DeviceIntSize, DevicePoint, EventLoopWaker, InputEvent, Key, KeyState, KeyboardEvent,
    LoadStatus, Location, Modifiers, MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, NamedKey,
    RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate,
    WebViewId, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;

/// Something the engine wants the shell to reflect.
#[derive(Debug)]
pub enum Update {
    Url(Url),
    History(Vec<Url>, usize),
    Title(Option<String>),
    Load(LoadStatus),
    Error(String),
    Fullscreen(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Delegate state. Servo calls the delegate from inside `spin_event_loop`, on the main thread.
#[derive(Default)]
struct Shared {
    dirty: RefCell<HashSet<WebViewId>>,
    updates: RefCell<Vec<(WebViewId, Update)>>,
}

impl WebViewDelegate for Shared {
    fn notify_new_frame_ready(&self, w: WebView) {
        self.dirty.borrow_mut().insert(w.id());
    }
    fn notify_url_changed(&self, w: WebView, url: Url) {
        self.updates.borrow_mut().push((w.id(), Update::Url(url)));
    }
    fn notify_history_changed(&self, w: WebView, entries: Vec<Url>, current: usize) {
        self.updates.borrow_mut().push((w.id(), Update::History(entries, current)));
    }
    fn notify_page_title_changed(&self, w: WebView, title: Option<String>) {
        self.updates.borrow_mut().push((w.id(), Update::Title(title)));
    }
    fn notify_load_status_changed(&self, w: WebView, status: LoadStatus) {
        self.updates.borrow_mut().push((w.id(), Update::Load(status)));
    }
    fn show_console_message(&self, _w: WebView, level: servo::ConsoleLogLevel, message: String) {
        eprintln!("[console {level:?}] {message}");
    }
    fn notify_fullscreen_state_changed(&self, w: WebView, fullscreen: bool) {
        self.updates.borrow_mut().push((w.id(), Update::Fullscreen(fullscreen)));
    }
    fn notify_crashed(&self, w: WebView, reason: String, _bt: Option<String>) {
        self.updates.borrow_mut().push((w.id(), Update::Error(reason)));
    }
}

// ---- wake-up channel: Servo (any thread) → iced subscription (main thread) ----

type Wake = (mpsc::UnboundedSender<()>, Mutex<Option<mpsc::UnboundedReceiver<()>>>);

fn wake_channel() -> &'static Wake {
    static WAKE: OnceLock<Wake> = OnceLock::new();
    WAKE.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded();
        (tx, Mutex::new(Some(rx)))
    })
}

static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

struct Waker;

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(Waker)
    }
    fn wake(&self) {
        if !WAKE_PENDING.swap(true, Ordering::AcqRel) {
            let _ = wake_channel().0.unbounded_send(());
        }
    }
}

/// Fires whenever Servo asks to be spun. Subscribe while an [`Engine`] exists.
pub fn wakes() -> Subscription<()> {
    Subscription::run(|| {
        // ponytail: single subscriber for the process lifetime; a second one would hang silently
        wake_channel().1.lock().unwrap().take().expect("engine wake subscription started twice")
    })
}

struct View {
    webview: WebView,
    ctx: Rc<SoftwareRenderingContext>,
    /// Last frame the shell allocated on the GPU. Held until the next one lands, so the widget never
    /// draws a handle that is still uploading (that gap is the gray flicker).
    frame: Option<image::Allocation>,
    uploading: bool,
    upload_failures: u8,
}

/// `BRO_TRACE`: JS evaluated in the active page every couple of seconds; prints `<video>` state.
const PROBE_JS: &str = r#"(()=>{const v=document.querySelector('video');if(!v)return 'no <video>';const r=[];for(let i=0;i<v.buffered.length;i++)r.push(v.buffered.start(i).toFixed(2)+'-'+v.buffered.end(i).toFixed(2));return JSON.stringify({rs:v.readyState,ns:v.networkState,t:+v.currentTime.toFixed(2),dur:v.duration,paused:v.paused,seeking:v.seeking,rate:v.playbackRate,ended:v.ended,err:v.error&&v.error.code,src:(v.currentSrc||'').slice(0,30),buf:r,w:v.videoWidth,h:v.videoHeight});})()"#;

pub struct Engine {
    servo: Servo,
    last_probe: std::time::Instant,
    shared: Rc<Shared>,
    views: HashMap<usize, View>,
    by_id: HashMap<WebViewId, usize>,
    size: PhysicalSize<u32>,
    scale: f32,
    active: Option<usize>,
}

impl Engine {
    pub fn new(size: PhysicalSize<u32>, scale: f32) -> Self {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        // Real sites use these without feature-detecting (a ReferenceError kills e.g. an Angular boot);
        // Servo ships them pref'd off.
        let prefs = servo::Preferences {
            dom_offscreen_canvas_enabled: true,
            dom_intersection_observer_enabled: true,
            dom_fontface_enabled: true,
            dom_adoptedstylesheet_enabled: true,
            ..Default::default()
        };
        let servo = ServoBuilder::default().preferences(prefs).event_loop_waker(Box::new(Waker)).build();
        servo.setup_logging(); // honours RUST_LOG
        Self {
            servo,
            last_probe: std::time::Instant::now(),
            shared: Rc::default(),
            views: HashMap::new(),
            by_id: HashMap::new(),
            size: clamp(size),
            scale,
            active: None,
        }
    }

    pub fn has(&self, tab: usize) -> bool {
        self.views.contains_key(&tab)
    }

    /// Existing view for `tab`, or a fresh one that starts at `url` (a `load()` issued right after
    /// `build()` loses to the builder's default about:blank navigation).
    fn view(&mut self, tab: usize, url: Option<Url>) -> &mut View {
        if !self.views.contains_key(&tab) {
            let ctx = Rc::new(SoftwareRenderingContext::new(self.size).expect("software GL context"));
            let mut builder = WebViewBuilder::new(&self.servo, ctx.clone())
                .hidpi_scale_factor(Scale::new(self.scale))
                .delegate(self.shared.clone());
            if let Some(u) = url {
                builder = builder.url(u);
            }
            let webview = builder.build();
            webview.hide();
            self.by_id.insert(webview.id(), tab);
            self.views.insert(tab, View { webview, ctx, frame: None, uploading: false, upload_failures: 0 });
        }
        self.views.get_mut(&tab).unwrap()
    }

    pub fn load(&mut self, tab: usize, url: Url) {
        if self.has(tab) {
            self.view(tab, None).webview.load(url);
        } else {
            self.view(tab, Some(url));
        }
    }
    pub fn close(&mut self, tab: usize) {
        if let Some(v) = self.views.remove(&tab) {
            self.by_id.remove(&v.webview.id());
            self.shared.dirty.borrow_mut().remove(&v.webview.id());
            if self.active == Some(tab) {
                self.active = None;
            }
        }
    }
    /// Only the selected tab needs compositing and CPU readback.
    pub fn activate(&mut self, tab: Option<usize>) {
        if self.active == tab {
            return;
        }
        if let Some(v) = self.active.and_then(|id| self.views.get(&id)) {
            v.webview.exit_fullscreen();
            v.webview.blur();
            v.webview.hide();
        }
        self.active = tab;
        if let Some(v) = tab.and_then(|id| self.views.get(&id)) {
            v.webview.resize(self.size);
            v.webview.set_hidpi_scale_factor(Scale::new(self.scale));
            v.webview.show();
            v.webview.focus();
            self.shared.dirty.borrow_mut().insert(v.webview.id());
        }
    }

    pub fn back(&self, tab: usize) {
        if let Some(v) = self.views.get(&tab) {
            v.webview.go_back(1);
        }
    }
    pub fn forward(&self, tab: usize) {
        if let Some(v) = self.views.get(&tab) {
            v.webview.go_forward(1);
        }
    }
    pub fn exit_fullscreen(&self, tab: usize) {
        if let Some(v) = self.views.get(&tab) { v.webview.exit_fullscreen(); }
    }
    pub fn reload(&self, tab: usize) {
        if let Some(v) = self.views.get(&tab) {
            v.webview.reload();
        }
    }
    /// (can_go_back, can_go_forward), if this tab is engine-backed.
    pub fn can_go(&self, tab: usize) -> Option<(bool, bool)> {
        self.views.get(&tab).map(|v| (v.webview.can_go_back(), v.webview.can_go_forward()))
    }
    pub fn set_zoom(&self, tab: usize, zoom: f32) {
        if let Some(v) = self.views.get(&tab) {
            v.webview.set_page_zoom(zoom);
        }
    }
    pub fn frame(&self, tab: usize) -> Option<image::Handle> {
        self.views.get(&tab).and_then(|v| v.frame.as_ref()).map(|a| a.handle().clone())
    }
    /// The shell finished uploading a frame from [`Engine::tick`].
    pub fn set_frame(&mut self, tab: usize, id: WebViewId, frame: Option<image::Allocation>) {
        if trace() {
            eprintln!("[trace] gpu tab={tab} success={}", frame.is_some());
        }
        if let Some(v) = self.views.get_mut(&tab).filter(|v| v.webview.id() == id) {
            v.uploading = false;
            if let Some(frame) = frame {
                v.frame = Some(frame);
                v.upload_failures = 0;
            } else {
                v.upload_failures = v.upload_failures.saturating_add(1);
                if v.upload_failures < 3 {
                    self.shared.dirty.borrow_mut().insert(id);
                } else {
                    self.shared
                        .updates
                        .borrow_mut()
                        .push((id, Update::Error("Could not upload the page image. Try reloading the page.".into())));
                }
            }
        }
    }

    /// Content area changed. All views share one size.
    pub fn resize(&mut self, size: PhysicalSize<u32>, scale: f32) {
        let size = clamp(size);
        if size == self.size && scale == self.scale {
            return;
        }
        let rescale = scale != self.scale;
        self.size = size;
        self.scale = scale;
        if let Some(v) = self.active.and_then(|id| self.views.get(&id)) {
            v.webview.resize(size);
            if rescale {
                v.webview.set_hidpi_scale_factor(Scale::new(scale));
            }
        }
    }

    /// Spin Servo, repaint dirty views. Returns shell-visible updates and freshly painted frames;
    /// the shell must GPU-allocate each frame and hand it back through [`Engine::set_frame`].
    pub fn tick(&mut self) -> (Vec<(usize, Update)>, Vec<(usize, WebViewId, image::Handle)>) {
        self.servo.spin_event_loop();
        if (trace() || video_smoke()) && self.last_probe.elapsed().as_secs() >= 2 {
            self.last_probe = std::time::Instant::now();
            for (tab, v) in &self.views {
                let tab = *tab;
                #[cfg(debug_assertions)]
                if video_smoke() {
                    v.webview.evaluate_javascript(include_str!("../tools/video/smoke.js"), |_| {});
                }
                v.webview.evaluate_javascript(PROBE_JS, move |result| match result {
                    Ok(value) => eprintln!("[probe] tab={tab} {value:?}"),
                    Err(error) => eprintln!("[probe] tab={tab} error {error:?}"),
                });
            }
        }
        let dirty: Vec<WebViewId> = self
            .active
            .and_then(|id| self.views.get(&id))
            .filter(|v| !v.uploading)
            .filter(|v| self.shared.dirty.borrow_mut().remove(&v.webview.id()))
            .map(|v| v.webview.id())
            .into_iter()
            .collect();
        let mut frames = Vec::new();
        for id in dirty {
            let Some(&tab) = self.by_id.get(&id) else { continue };
            let v = self.views.get_mut(&tab).unwrap();
            v.webview.paint();
            let s = v.ctx.size();
            let rect = DeviceIntRect::from_size(DeviceIntSize::new(s.width as i32, s.height as i32));
            if let Some(img) = v.ctx.read_to_image(rect) {
                if trace() {
                    let (w, h) = (img.width(), img.height());
                    eprintln!(
                        "[trace] read tab={tab} {w}x{h} mid={:?} corner={:?}",
                        img.get_pixel(w / 2, h / 2).0,
                        img.get_pixel(0, 0).0
                    );
                }
                v.uploading = true;
                frames.push((tab, id, image::Handle::from_rgba(img.width(), img.height(), img.into_raw())));
            }
        }
        let ups = std::mem::take(&mut *self.shared.updates.borrow_mut());
        let ups = ups.into_iter().filter_map(|(id, u)| self.by_id.get(&id).map(|&t| (t, u))).collect();
        (ups, frames)
    }

    // ---- input (device pixels, relative to the content area) ----

    pub fn mouse_move(&self, tab: usize, x: f32, y: f32) {
        if let Some(v) = self.views.get(&tab) {
            v.webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(DevicePoint::new(x, y).into())));
        }
    }
    pub fn mouse_button(&self, tab: usize, button: Button, down: bool, x: f32, y: f32) {
        let Some(v) = self.views.get(&tab) else { return };
        let button = match button {
            Button::Left => MouseButton::Primary,
            Button::Right => MouseButton::Secondary,
            Button::Middle => MouseButton::Auxiliary,
        };
        let action = if down { MouseButtonAction::Down } else { MouseButtonAction::Up };
        v.webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
            action,
            button,
            DevicePoint::new(x, y).into(),
        )));
    }
    pub fn wheel(&self, tab: usize, delta: mouse::ScrollDelta, x: f32, y: f32) {
        let Some(v) = self.views.get(&tab) else { return };
        // same constants servoshell uses
        let (dx, dy, mode) = match delta {
            mouse::ScrollDelta::Lines { x, y } => ((x * 76.0) as f64, (y * 76.0) as f64, WheelMode::DeltaPixel),
            mouse::ScrollDelta::Pixels { x, y } => {
                ((x * self.scale) as f64, (y * self.scale) as f64, WheelMode::DeltaPixel)
            }
        };
        let delta = WheelDelta { x: dx, y: dy, z: 0.0, mode };
        v.webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(delta, DevicePoint::new(x, y).into())));
    }
    pub fn key(&self, tab: usize, down: bool, repeat: bool, key: &keyboard::Key, mods: keyboard::Modifiers) {
        let Some(v) = self.views.get(&tab) else { return };
        let state = if down { KeyState::Down } else { KeyState::Up };
        let ev = KeyboardEvent::new_without_event(
            state,
            to_kt(key),
            Code::Unidentified,
            Location::Standard,
            to_kt_mods(mods),
            repeat,
            false,
        );
        v.webview.notify_input_event(InputEvent::Keyboard(ev));
    }
}

/// Clear before spinning, so a wake arriving during the spin queues another turn.
pub fn acknowledge_wake() {
    WAKE_PENDING.store(false, Ordering::Release);
}

/// `BRO_TRACE=1` prints frame plumbing to stderr.
fn trace() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BRO_TRACE").is_some())
}

fn video_smoke() -> bool {
    cfg!(debug_assertions) && std::env::var_os("BRO_VIDEO_SMOKE").is_some()
}

fn clamp(s: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(s.width.max(1), s.height.max(1))
}

/// iced key → keyboard_types key. Unmapped named keys become `Unidentified`.
pub fn to_kt(key: &keyboard::Key) -> Key {
    use keyboard::key::Named as N;
    match key {
        keyboard::Key::Character(s) => Key::Character(s.to_string()),
        keyboard::Key::Named(N::Space) => Key::Character(" ".into()),
        keyboard::Key::Named(n) => Key::Named(match n {
            N::Enter => NamedKey::Enter,
            N::Tab => NamedKey::Tab,
            N::Backspace => NamedKey::Backspace,
            N::Delete => NamedKey::Delete,
            N::Escape => NamedKey::Escape,
            N::ArrowUp => NamedKey::ArrowUp,
            N::ArrowDown => NamedKey::ArrowDown,
            N::ArrowLeft => NamedKey::ArrowLeft,
            N::ArrowRight => NamedKey::ArrowRight,
            N::Home => NamedKey::Home,
            N::End => NamedKey::End,
            N::PageUp => NamedKey::PageUp,
            N::PageDown => NamedKey::PageDown,
            N::Shift => NamedKey::Shift,
            N::Control => NamedKey::Control,
            N::Alt => NamedKey::Alt,
            N::Meta | N::Super => NamedKey::Meta,
            _ => NamedKey::Unidentified,
        }),
        _ => Key::Named(NamedKey::Unidentified),
    }
}

fn to_kt_mods(m: keyboard::Modifiers) -> Modifiers {
    let mut out = Modifiers::empty();
    out.set(Modifiers::SHIFT, m.shift());
    out.set(Modifiers::CONTROL, m.control());
    out.set(Modifiers::ALT, m.alt());
    out.set(Modifiers::META, m.logo());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping() {
        use keyboard::key::Named;
        assert_eq!(to_kt(&keyboard::Key::Character("a".into())), Key::Character("a".into()));
        assert_eq!(to_kt(&keyboard::Key::Named(Named::Enter)), Key::Named(NamedKey::Enter));
        assert_eq!(to_kt(&keyboard::Key::Named(Named::Space)), Key::Character(" ".into()));
        assert_eq!(to_kt(&keyboard::Key::Named(Named::F1)), Key::Named(NamedKey::Unidentified));
        let m = to_kt_mods(keyboard::Modifiers::SHIFT | keyboard::Modifiers::LOGO);
        assert!(m.contains(Modifiers::SHIFT | Modifiers::META) && !m.contains(Modifiers::ALT));
    }
}
