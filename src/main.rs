//! bro — Servo browser with an iced shell.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use dpi::PhysicalSize;
use iced::animation::{Animation, Easing};
use iced::keyboard::{self, Key};
use iced::widget::image;
use iced::widget::{
    button, column, container, mouse_area, operation, row, scrollable, space, stack, svg, text, text_input,
};
use iced::{
    Border, Color, Element, Event, Fill, Shadow, Size, Subscription, Task, Theme, Vector, event, mouse, window,
};
use url::Url;

mod engine;

// Helium layout constants (px).
const BASE: f32 = 3.0;
const TAB_H: f32 = 31.0;
const TOOLBAR_BTN: f32 = 28.0;
const OMNIBOX_H: f32 = 28.0;
const OMNIBOX_MARGIN: f32 = 6.0;
const TAB_RADIUS: f32 = 8.0;
const OMNIBOX_RADIUS: f32 = 6.0;
const SIDEBAR_W: f32 = 240.0;
const TRAFFIC_LIGHTS_W: f32 = 76.0;
const MENU_W: f32 = 300.0;
const MENU_RADIUS: f32 = 8.0;

const OMNIBOX_ID: &str = "omnibox";
const ZOOM_STEPS: [u16; 17] = [25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500];
const PROFILE_COLORS: [Color; 6] = [
    c(0x8b, 0x7c, 0xf6),
    c(0x34, 0xc7, 0x59),
    c(0xff, 0x9f, 0x0a),
    c(0xff, 0x45, 0x5a),
    c(0x0a, 0x84, 0xff),
    c(0xff, 0x2d, 0x92),
];

fn main() -> iced::Result {
    iced::application(
        || {
            let mut boot = vec![
                titlebar::init(),
                window::latest().and_then(window::size).map(Message::Resized),
                window::latest().and_then(window::scale_factor).map(Message::Scale),
            ];
            // `bro <url>` opens straight into a page
            if let Some(url) = std::env::args().nth(1) {
                boot.push(Task::done(Message::UrlInput(url)).chain(Task::done(Message::UrlSubmit)));
            }
            (Browser::default(), Task::batch(boot))
        },
        update,
        view,
    )
    .title("bro")
    .theme(|b: &Browser| if b.dark { Theme::Dark } else { Theme::Light })
    .subscription(subscription)
    .window(window::Settings {
        size: (1200.0, 800.0).into(),
        icon: app_icon(),
        // toolbar doubles as the titlebar; traffic lights float over it
        #[cfg(target_os = "macos")]
        platform_specific: window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        ..Default::default()
    })
    .run()
}

/// macOS: the transparent titlebar still sits over the toolbar and zooms on any double-click
/// (even on our buttons). Make it hit-test only its traffic lights so everything else reaches iced.
#[cfg(target_os = "macos")]
mod titlebar {
    use iced::window::{self, raw_window_handle::RawWindowHandle};
    use objc2::encode::{Encode, Encoding};
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{ffi, msg_send, sel};
    use std::sync::OnceLock;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point(f64, f64);
    unsafe impl Encode for Point {
        const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }
    type HitTest = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Point) -> *mut AnyObject;
    static ORIGINAL: OnceLock<HitTest> = OnceLock::new();

    unsafe extern "C-unwind" fn hit_test(this: *mut AnyObject, sel: Sel, p: Point) -> *mut AnyObject {
        let hit = unsafe { ORIGINAL.get().unwrap()(this, sel, p) };
        // walk up from the hit view: keep it only if it's inside a button (the traffic lights)
        let button = AnyClass::get(c"NSButton").unwrap();
        let mut v = hit;
        while !v.is_null() && v != this {
            if unsafe { msg_send![v, isKindOfClass: button] } {
                return hit;
            }
            v = unsafe { msg_send![v, superview] };
        }
        std::ptr::null_mut()
    }

    pub fn init<T: Send + 'static>() -> iced::Task<T> {
        window::latest().and_then(|id| window::run(id, |w| patch(w))).discard()
    }

    fn patch(w: &dyn window::Window) {
        let Ok(RawWindowHandle::AppKit(h)) = w.window_handle().map(|h| h.as_raw()) else { return };
        unsafe {
            let view = h.ns_view.as_ptr() as *mut AnyObject;
            let win: *mut AnyObject = msg_send![view, window];
            let close: *mut AnyObject = msg_send![win, standardWindowButton: 0usize]; // NSWindowCloseButton
            if close.is_null() {
                return;
            }
            let bar: *mut AnyObject = msg_send![close, superview]; // NSTitlebarView
            let container: *mut AnyObject = msg_send![bar, superview]; // NSTitlebarContainerView
            if container.is_null() || ORIGINAL.get().is_some() {
                return;
            }
            let cls = (*container).class();
            let Some(m) = cls.instance_method(sel!(hitTest:)) else { return };
            let _ = ORIGINAL.set(std::mem::transmute::<Imp, HitTest>(m.implementation()));
            let imp = std::mem::transmute::<HitTest, Imp>(hit_test);
            ffi::class_replaceMethod(cls as *const _ as *mut _, sel!(hitTest:), imp, c"@@:{CGPoint=dd}".as_ptr());
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod titlebar {
    pub fn init<T>() -> iced::Task<T> {
        iced::Task::none()
    }
}

/// Window/taskbar icon (Windows, Linux).
fn app_icon() -> Option<window::Icon> {
    let mut r = png::Decoder::new(&include_bytes!("../assets/icon-256.png")[..]).read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    window::icon::from_rgba(buf, info.width, info.height).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Popup {
    None,
    Menu,
    Profiles,
}

#[derive(Debug, Clone)]
struct Tab {
    id: usize,
    title: String,
    url: String,
    /// Omnibox text; may differ from `url` while typing.
    input: String,
    history: Vec<String>,
    /// Recreated views navigate the retained URL history in the shell.
    shell_history: bool,
    /// Index into `history` for the current page.
    cursor: usize,
    zoom: u16,
    incognito: bool,
    loading: bool,
    error: Option<String>,
}

impl Tab {
    fn new(id: usize) -> Self {
        Self {
            id,
            title: "New Tab".into(),
            url: String::new(),
            input: String::new(),
            history: Vec::new(),
            shell_history: false,
            cursor: 0,
            zoom: 100,
            incognito: false,
            loading: false,
            error: None,
        }
    }
    fn can_back(&self) -> bool {
        self.cursor > 0
    }
    fn can_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }
    fn goto(&mut self, url: String) {
        self.history.truncate(self.cursor + if self.history.is_empty() { 0 } else { 1 });
        self.history.push(url.clone());
        self.cursor = self.history.len() - 1;
        self.set_current(url);
    }
    fn set_current(&mut self, url: String) {
        self.title = page_title(&url);
        self.input = url.clone();
        self.url = url;
    }
    fn receive_url(&mut self, url: Url) {
        let editing = self.input != self.url;
        let same_url = Url::parse(&self.url).is_ok_and(|old| old == url);
        if self.shell_history {
            if same_url && !self.history.is_empty() {
                self.history[self.cursor] = url.to_string();
            } else if !same_url {
                self.history.truncate(self.cursor + usize::from(!self.history.is_empty()));
                self.history.push(url.to_string());
                self.cursor = self.history.len() - 1;
            }
        }
        self.url = url.to_string();
        if !editing {
            self.input = self.url.clone();
        }
    }
    fn zoom_step(&mut self, dir: i8) {
        let i = ZOOM_STEPS.iter().position(|&z| z >= self.zoom).unwrap_or(7);
        let j = (i as i32 + dir as i32).clamp(0, ZOOM_STEPS.len() as i32 - 1) as usize;
        self.zoom = ZOOM_STEPS[j];
    }
}

/// `bro://settings` → "Settings"; https://x.y/z → "x.y/z".
fn page_title(url: &str) -> String {
    if let Some(p) = url.strip_prefix("bro://") {
        let mut s = p.replace('-', " ");
        if let Some(f) = s.get_mut(0..1) {
            f.make_ascii_uppercase();
        }
        s
    } else {
        url.trim_start_matches("https://").trim_start_matches("http://").to_string()
    }
}

/// One profile = one tab session.
#[derive(Debug, Clone)]
struct Profile {
    name: String,
    color: Color,
    tabs: Vec<Tab>,
    active: usize,
    closed: Vec<Tab>,
    bookmarks: Vec<(String, String)>,
}

impl Profile {
    fn new(name: &str, color: Color, first_tab_id: usize) -> Self {
        Self {
            name: name.into(),
            color,
            tabs: vec![Tab::new(first_tab_id)],
            active: 0,
            closed: Vec::new(),
            bookmarks: Vec::new(),
        }
    }
    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
}

/// Animation key: (kind, discriminator). Kinds: "hover", "active", "appear", "en".
type Key2 = (&'static str, usize);

fn anim(on: bool) -> Animation<bool> {
    Animation::new(on).quick().easing(Easing::EaseOutCubic)
}

struct Browser {
    profiles: Vec<Profile>,
    profile: usize,
    next_id: usize,
    /// Vertical tab sidebar visible.
    sidebar: bool,
    dark: bool,
    popup: Popup,
    fullscreen: bool,
    page_fullscreen: Option<usize>,
    fullscreen_restore: bool,
    /// pressed on empty toolbar; drag starts on first move so plain clicks never reach AppKit's drag (it zooms on clickCount=2)
    drag_armed: bool,
    // ---- animation state ----
    now: Instant,
    anims: HashMap<Key2, Animation<bool>>,
    theme_a: Animation<bool>,
    popup_a: Animation<bool>,
    /// Popup being drawn; lags `popup` while the close fade plays.
    shown: Popup,
    sidebar_a: Animation<bool>,
    /// Restarted on every navigation / tab / profile switch.
    content_a: Animation<bool>,
    /// (profile, tab id, url) last seen, to detect content changes.
    content_key: (usize, usize, String),
    // ---- engine ----
    engine: Option<engine::Engine>,
    /// Logical window size and scale factor; the page area is derived from these.
    win: Size,
    scale: f32,
    /// Last pointer position over the page, device px.
    cursor: (f32, f32),
    /// Keys whose key-down went to the page; only their key-up is forwarded (the omnibox eats the
    /// down of keys it handles, but never the up).
    keys_down: HashSet<keyboard::Key>,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            profiles: vec![Profile::new("Default", PROFILE_COLORS[0], 0)],
            profile: 0,
            next_id: 1,
            sidebar: true,
            dark: true,
            popup: Popup::None,
            fullscreen: false,
            page_fullscreen: None,
            fullscreen_restore: false,
            drag_armed: false,
            now: Instant::now(),
            anims: HashMap::from([(("active", 0), anim(true))]),
            theme_a: anim(true).slow(),
            popup_a: anim(false).duration(std::time::Duration::from_millis(160)),
            shown: Popup::None,
            sidebar_a: anim(true),
            content_a: anim(true).duration(std::time::Duration::from_millis(260)),
            content_key: (0, 0, String::new()),
            engine: None,
            win: Size::new(1200.0, 800.0),
            scale: 1.0,
            cursor: (0.0, 0.0),
            keys_down: HashSet::new(),
        }
    }
}

impl Browser {
    fn p(&self) -> &Profile {
        &self.profiles[self.profile]
    }
    fn pm(&mut self) -> &mut Profile {
        &mut self.profiles[self.profile]
    }
    fn open(&mut self, url: Option<&str>) -> Task<Message> {
        let mut t = Tab::new(self.next_id);
        self.next_id += 1;
        if let Some(u) = url {
            t.goto(u.into());
        }
        let id = t.id;
        let p = self.pm();
        p.tabs.push(t);
        p.active = p.tabs.len() - 1;
        let now = self.now;
        self.anims.insert(("appear", id), anim(false).go(true, now));
        operation::focus(OMNIBOX_ID)
    }

    /// Page area in device pixels: window minus toolbar, minus sidebar when shown.
    fn content_size(&self) -> PhysicalSize<u32> {
        if self.page_fullscreen == Some(self.p().tab().id) {
            return PhysicalSize::new((self.win.width * self.scale).max(1.0) as u32,
                (self.win.height * self.scale).max(1.0) as u32);
        }
        let w = self.win.width - if self.sidebar { SIDEBAR_W + 1.0 } else { 0.0 };
        let h = self.win.height - (TOOLBAR_BTN + 2.0 * BASE + 1.0);
        PhysicalSize::new((w * self.scale).round().max(1.0) as u32, (h * self.scale).round().max(1.0) as u32)
    }
    /// Boots Servo on first use, so the shell starts instantly and works without it.
    fn engine(&mut self) -> &mut engine::Engine {
        if self.engine.is_none() {
            self.engine = Some(engine::Engine::new(self.content_size(), self.scale));
        }
        self.engine.as_mut().unwrap()
    }
    fn resize_engine(&mut self) {
        let (size, scale) = (self.content_size(), self.scale);
        if let Some(e) = self.engine.as_mut() {
            e.resize(size, scale);
        }
    }
    fn push_zoom(&self) {
        let t = self.p().tab();
        if let Some(e) = &self.engine {
            e.set_zoom(t.id, t.zoom as f32 / 100.0);
        }
    }
    /// Spin the engine and reflect its updates into tabs. Runs after every message.
    /// Returns the GPU uploads for any new frames.
    fn pump(&mut self) -> Task<Message> {
        let active = self.p().tab().id;
        let mut fullscreen_changed = false;
        if self.page_fullscreen.is_some_and(|id| id != active) {
            self.page_fullscreen = None;
            self.fullscreen = self.fullscreen_restore;
            fullscreen_changed = true;
        }
        let Some(e) = self.engine.as_mut() else { return Task::none() };
        e.activate(e.has(active).then_some(active));
        let (updates, frames) = e.tick();
        let uploads = frames
            .into_iter()
            .map(|(tab, id, handle)| image::allocate(handle).map(move |r| Message::Allocated(tab, id, r.ok())));
        for (id, up) in updates {
            let Some(t) = self.profiles.iter_mut().flat_map(|p| p.tabs.iter_mut()).find(|t| t.id == id) else {
                continue;
            };
            match up {
                engine::Update::Url(u) => t.receive_url(u),
                engine::Update::History(entries, current) => {
                    if !t.shell_history && current < entries.len() {
                        t.history = entries.into_iter().map(|u| u.to_string()).collect();
                        t.cursor = current;
                    }
                }
                engine::Update::Title(title) => {
                    let fallback = page_title(&t.url);
                    t.title = title.filter(|s| !s.trim().is_empty()).unwrap_or(fallback);
                }
                engine::Update::Load(status) => {
                    t.loading = status != servo::LoadStatus::Complete;
                    if status == servo::LoadStatus::Started {
                        t.error = None;
                    }
                }
                engine::Update::Fullscreen(fullscreen) => {
                    if id == active {
                        if fullscreen && self.page_fullscreen.is_none() {
                            self.fullscreen_restore = self.fullscreen;
                        }
                        self.page_fullscreen = fullscreen.then_some(id);
                        self.fullscreen = fullscreen || self.fullscreen_restore;
                        fullscreen_changed = true;
                    }
                }
                engine::Update::Error(reason) => {
                    t.loading = false;
                    t.error = Some(reason);
                }
            }
        }
        let uploads = Task::batch(uploads);
        if fullscreen_changed {
            self.resize_engine();
            let mode = if self.fullscreen { window::Mode::Fullscreen } else { window::Mode::Windowed };
            Task::batch([uploads, window::latest().and_then(move |id| window::set_mode(id, mode))])
        } else { uploads }
    }
    /// (back, forward) availability for the active tab; the engine owns history for web tabs.
    fn can_nav(&self) -> (bool, bool) {
        let t = self.p().tab();
        if t.shell_history {
            return (t.can_back(), t.can_forward());
        }
        self.engine.as_ref().and_then(|e| e.can_go(t.id)).unwrap_or((t.can_back(), t.can_forward()))
    }
    /// URLs the engine handles; everything else is a `bro://` shell page.
    fn is_web(url: &str) -> bool {
        url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")
    }

    /// Interpolated 0..1 for `key`; missing keys sit at their resting value.
    fn t(&self, key: Key2) -> f32 {
        match self.anims.get(&key) {
            Some(a) => a.interpolate(0.0, 1.0, self.now),
            None => {
                if key.0 == "appear" {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
    fn set(&mut self, key: Key2, on: bool) {
        let now = self.now;
        let a = self.anims.entry(key).or_insert_with(|| anim(!on));
        if a.value() != on {
            a.go_mut(on, now);
        }
    }
    fn animating(&self) -> bool {
        let n = self.now;
        self.anims.values().any(|a| a.is_animating(n))
            || [&self.theme_a, &self.popup_a, &self.sidebar_a, &self.content_a].iter().any(|a| a.is_animating(n))
    }
    /// Point every animation at the current logical state. Runs after each update.
    fn sync(&mut self) {
        let now = self.now;
        let go = |a: &mut Animation<bool>, v: bool| {
            if a.value() != v {
                a.go_mut(v, now)
            }
        };
        go(&mut self.theme_a, self.dark);
        go(&mut self.sidebar_a, self.sidebar);

        // popup: switch panel instantly if another is open, else fade
        if self.popup != Popup::None {
            if self.shown != self.popup {
                if self.shown != Popup::None {
                    self.popup_a = anim(false).duration(std::time::Duration::from_millis(160));
                }
                self.shown = self.popup;
                self.clear_hover();
            }
            go(&mut self.popup_a, true);
        } else if self.popup_a.value() {
            go(&mut self.popup_a, false);
            self.clear_hover();
        }

        let (can_back, can_fwd) = self.can_nav();
        let (active_id, url) = {
            let t = self.p().tab();
            (t.id, t.url.clone())
        };
        let stale: Vec<Key2> = self.anims.keys().filter(|k| k.0 == "active" && k.1 != active_id).copied().collect();
        for k in stale {
            self.set(k, false);
        }
        self.set(("active", active_id), true);
        self.set(("en", 0), can_back);
        self.set(("en", 1), can_fwd);

        let key = (self.profile, active_id, url);
        if key != self.content_key {
            self.content_key = key;
            self.content_a = anim(false).duration(std::time::Duration::from_millis(260)).go(true, now);
        }
    }
    fn clear_hover(&mut self) {
        let now = self.now;
        for (k, a) in self.anims.iter_mut() {
            if k.0 == "hover" && a.value() {
                a.go_mut(false, now);
            }
        }
    }
    /// Drop finished animations that rest at their default, so the map stays small.
    fn gc(&mut self) {
        let now = self.now;
        self.anims.retain(|k, a| {
            a.is_animating(now)
                || match k.0 {
                    "appear" => !a.value(),
                    "hover" => a.value(),
                    _ => true,
                }
        });
    }
}

/// App menu entries (⋮). Order = visual order; `Divider` draws a separator, `Zoom` the −/100%/+ row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    NewTab,
    NewWindow,
    Incognito,
    Divider1,
    History,
    Downloads,
    Bookmarks,
    TabGroups,
    Extensions,
    DeleteData,
    Divider2,
    Zoom,
    Divider3,
    Print,
    FindEdit,
    CastSaveShare,
    MoreTools,
    Divider4,
    About,
    Customize,
    Settings,
}

impl MenuItem {
    const ALL: [MenuItem; 21] = [
        Self::NewTab,
        Self::NewWindow,
        Self::Incognito,
        Self::Divider1,
        Self::History,
        Self::Downloads,
        Self::Bookmarks,
        Self::TabGroups,
        Self::Extensions,
        Self::DeleteData,
        Self::Divider2,
        Self::Zoom,
        Self::Divider3,
        Self::Print,
        Self::FindEdit,
        Self::CastSaveShare,
        Self::MoreTools,
        Self::Divider4,
        Self::About,
        Self::Customize,
        Self::Settings,
    ];

    /// (icon, label, shortcut, has_submenu)
    fn spec(self) -> (&'static str, &'static str, &'static str, bool) {
        match self {
            Self::NewTab => ("square-plus", "New Tab", "⌘T", false),
            Self::NewWindow => ("app-window", "New Window", "⌘N", false),
            Self::Incognito => ("venetian-mask", "New Incognito Window", "⇧⌘N", false),
            Self::History => ("clock-arrow-left", "History", "", true),
            Self::Downloads => ("download", "Downloads", "⌥⌘L", false),
            Self::Bookmarks => ("star", "Bookmarks", "", true),
            Self::TabGroups => ("layout-grid", "Tab Groups", "", true),
            Self::Extensions => ("puzzle", "Extensions", "", false),
            Self::DeleteData => ("trash", "Delete Browsing Data…", "⇧⌘⌫", false),
            Self::Print => ("printer", "Print…", "⌘P", false),
            Self::FindEdit => ("search", "Find and Edit", "", true),
            Self::CastSaveShare => ("share", "Cast, Save, and Share", "", true),
            Self::MoreTools => ("wrench", "More Tools", "", true),
            Self::About => ("info", "About bro", "", false),
            Self::Customize => ("paintbrush", "Customize bro", "", false),
            Self::Settings => ("settings", "Settings", "⌘,", false),
            Self::Divider1 | Self::Divider2 | Self::Divider3 | Self::Divider4 | Self::Zoom => ("", "", "", false),
        }
    }

    /// Internal page this item opens, if any. Everything else is an engine hook for later.
    fn page(self) -> Option<&'static str> {
        Some(match self {
            Self::History => "bro://history",
            Self::Downloads => "bro://downloads",
            Self::Bookmarks => "bro://bookmarks",
            Self::TabGroups => "bro://tab-groups",
            Self::Extensions => "bro://extensions",
            Self::DeleteData => "bro://delete-browsing-data",
            Self::FindEdit => "bro://find",
            Self::CastSaveShare => "bro://share",
            Self::MoreTools => "bro://tools",
            Self::About => "bro://about",
            Self::Customize => "bro://customize",
            Self::Settings => "bro://settings",
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
enum Message {
    Noop,
    ArmDrag(bool),
    DragWindow,
    ZoomWindow,
    NewTab,
    ReopenTab,
    Bookmark,
    Navigate(String),
    CloseTab(usize),
    CloseActive,
    SelectTab(usize),
    SelectLastTab,
    UrlInput(String),
    UrlSubmit,
    FocusOmnibox,
    Back,
    Forward,
    Reload,
    ToggleSidebar,
    ToggleTheme,
    TogglePopup(Popup),
    ClosePopup,
    Menu(MenuItem),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleFullscreen,
    ExitPageFullscreen,
    SelectProfile(usize),
    AddProfile,
    Hover(Key2, bool),
    Frame(Instant),
    Resized(Size),
    Scale(f32),
    /// Servo asked to be spun; the spin itself happens in `pump`.
    Wake,
    WebMove(iced::Point),
    WebButton(engine::Button, bool),
    WebScroll(mouse::ScrollDelta),
    WebKey(bool, keyboard::Key, keyboard::Modifiers),
    ReleaseKeys,
    /// A page frame finished uploading to the GPU.
    Allocated(usize, servo::WebViewId, Option<image::Allocation>),
}

fn unfocus_widgets() -> Task<Message> {
    iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::unfocus())
}

fn update(b: &mut Browser, m: Message) -> Task<Message> {
    b.now = if let Message::Frame(t) = m { t } else { Instant::now() };
    let old_tab = b.p().tab().id;
    let release_keys = matches!(m, Message::FocusOmnibox | Message::TogglePopup(_) | Message::ReleaseKeys);
    let task = apply(b, m);
    if release_keys || b.p().tab().id != old_tab {
        for key in b.keys_down.drain() {
            if let Some(e) = &b.engine {
                e.key(old_tab, false, false, &key, keyboard::Modifiers::empty());
            }
        }
    }
    let uploads = b.pump();
    b.sync();
    Task::batch([task, uploads])
}

fn apply(b: &mut Browser, m: Message) -> Task<Message> {
    match m {
        Message::Hover(k, on) => b.set(k, on),
        Message::Frame(_) => b.gc(),
        Message::Noop | Message::ReleaseKeys => {}
        Message::ArmDrag(on) => b.drag_armed = on,
        Message::DragWindow => {
            if !std::mem::take(&mut b.drag_armed) {
                return Task::none();
            }
            return window::latest().and_then(window::drag);
        }
        Message::ZoomWindow => return window::latest().and_then(window::toggle_maximize),
        Message::NewTab => return b.open(None),
        Message::Navigate(url) => {
            b.pm().tab_mut().input = url;
            return apply(b, Message::UrlSubmit);
        }
        Message::Bookmark => {
            let t = b.p().tab();
            if !Browser::is_web(&t.url) || t.incognito {
                return Task::none();
            }
            let entry = (t.title.clone(), t.url.clone());
            let bookmarks = &mut b.pm().bookmarks;
            if let Some(i) = bookmarks.iter().position(|(_, url)| url == &entry.1) {
                bookmarks.remove(i);
            } else {
                bookmarks.push(entry);
            }
        }
        Message::ReopenTab => {
            if let Some(mut t) = b.pm().closed.pop() {
                t.input = t.url.clone();
                t.shell_history = true;
                t.id = b.next_id;
                b.next_id += 1;
                t.loading = false;
                t.error = None;
                let (id, url, zoom) = (t.id, t.url.clone(), t.zoom);
                let p = b.pm();
                p.tabs.push(t);
                p.active = p.tabs.len() - 1;
                if Browser::is_web(&url) {
                    if let Ok(url) = Url::parse(&url) {
                        b.engine().load(id, url);
                        b.engine().set_zoom(id, zoom as f32 / 100.0);
                    }
                }
            }
        }
        Message::CloseActive => return apply(b, Message::CloseTab(b.p().tab().id)),
        Message::CloseTab(id) => {
            let p = b.pm();
            if !p.tabs.iter().any(|t| t.id == id) {
                return Task::none();
            }
            if p.tabs.len() == 1 {
                return iced::exit();
            }
            if let Some(i) = p.tabs.iter().position(|t| t.id == id) {
                let closed = p.tabs.remove(i);
                if !closed.incognito {
                    if p.closed.len() == 20 {
                        p.closed.remove(0);
                    }
                    p.closed.push(closed);
                }
                if p.active >= p.tabs.len() {
                    p.active = p.tabs.len() - 1;
                } else if i < p.active {
                    p.active -= 1;
                }
            }
            b.anims.retain(|k, _| !(k.1 == id && matches!(k.0, "active" | "appear")));
            if let Some(e) = b.engine.as_mut() {
                e.close(id);
            }
        }
        Message::SelectTab(i) => {
            let p = b.pm();
            p.active = i.min(p.tabs.len() - 1);
        }
        Message::SelectLastTab => b.pm().active = b.p().tabs.len() - 1,
        Message::UrlInput(s) => b.pm().tab_mut().input = s,
        Message::UrlSubmit => {
            let raw = b.p().tab().input.trim().to_string();
            if raw.is_empty() {
                return Task::none();
            }
            let url = normalize(&raw);
            let id = b.p().tab().id;
            if Browser::is_web(&url) {
                let Ok(parsed) = Url::parse(&url) else { return Task::none() };
                b.engine().load(id, parsed);
                b.push_zoom();
            } else if let Some(e) = &mut b.engine {
                e.close(id);
            }
            let t = b.pm().tab_mut();
            t.shell_history |= !Browser::is_web(&url);
            t.loading = Browser::is_web(&url);
            t.error = None;
            t.goto(url);
            return unfocus_widgets();
        }
        Message::FocusOmnibox => return operation::focus(OMNIBOX_ID),
        Message::Back => {
            let id = b.p().tab().id;
            if !b.p().tab().shell_history && b.engine.as_ref().is_some_and(|e| e.has(id)) {
                b.engine().back(id);
                return Task::none();
            }
            let t = b.pm().tab_mut();
            if t.can_back() {
                t.cursor -= 1;
                t.set_current(t.history[t.cursor].clone());
                let (id, url, zoom) = (t.id, t.url.clone(), t.zoom);
                if Browser::is_web(&url) {
                    if let Ok(url) = Url::parse(&url) {
                        b.engine().load(id, url);
                        b.engine().set_zoom(id, zoom as f32 / 100.0);
                    }
                } else if let Some(e) = &mut b.engine {
                    e.close(id);
                }
            }
        }
        Message::Forward => {
            let id = b.p().tab().id;
            if !b.p().tab().shell_history && b.engine.as_ref().is_some_and(|e| e.has(id)) {
                b.engine().forward(id);
                return Task::none();
            }
            let t = b.pm().tab_mut();
            if t.can_forward() {
                t.cursor += 1;
                t.set_current(t.history[t.cursor].clone());
                let (id, url, zoom) = (t.id, t.url.clone(), t.zoom);
                if Browser::is_web(&url) {
                    if let Ok(url) = Url::parse(&url) {
                        b.engine().load(id, url);
                        b.engine().set_zoom(id, zoom as f32 / 100.0);
                    }
                } else if let Some(e) = &mut b.engine {
                    e.close(id);
                }
            }
        }
        Message::Reload => {
            let id = b.p().tab().id;
            if let Some(e) = &b.engine {
                e.reload(id);
            }
        }
        Message::ToggleSidebar => {
            b.sidebar = !b.sidebar;
            b.resize_engine();
        }
        Message::Resized(s) => {
            b.win = s;
            b.resize_engine();
        }
        Message::Scale(s) => {
            b.scale = s;
            b.resize_engine();
        }
        Message::Wake => engine::acknowledge_wake(),
        Message::WebMove(pt) => {
            b.cursor = (pt.x * b.scale, pt.y * b.scale);
            let id = b.p().tab().id;
            if let Some(e) = &b.engine {
                e.mouse_move(id, b.cursor.0, b.cursor.1);
            }
        }
        Message::WebButton(btn, down) => {
            let id = b.p().tab().id;
            if let Some(e) = &b.engine {
                e.mouse_button(id, btn, down, b.cursor.0, b.cursor.1);
            }
            if down {
                return unfocus_widgets();
            }
        }
        Message::WebScroll(d) => {
            let id = b.p().tab().id;
            if let Some(e) = &b.engine {
                e.wheel(id, d, b.cursor.0, b.cursor.1);
            }
        }
        Message::WebKey(down, key, mods) => {
            let repeat = down && b.keys_down.contains(&key);
            let deliver = if down {
                b.keys_down.insert(key.clone());
                true
            } else {
                b.keys_down.remove(&key)
            };
            let id = b.p().tab().id;
            if let (true, Some(e)) = (deliver, &b.engine) {
                e.key(id, down, repeat, &key, mods);
            }
        }
        Message::Allocated(tab, id, frame) => {
            if let Some(e) = b.engine.as_mut() {
                e.set_frame(tab, id, frame);
            }
        }
        Message::ToggleTheme => b.dark = !b.dark,
        Message::TogglePopup(p) => b.popup = if b.popup == p { Popup::None } else { p },
        Message::ClosePopup => b.popup = Popup::None,
        Message::Menu(item) => {
            b.popup = Popup::None;
            match item {
                MenuItem::NewTab => return b.open(None),
                MenuItem::NewWindow => return b.open(None), // ponytail: multi-window needs iced::daemon; new tab for now
                MenuItem::Incognito => {
                    return b.open(Some("bro://private-browsing"));
                }
                MenuItem::Print => {} // engine hook later
                _ => {
                    if let Some(url) = item.page() {
                        return b.open(Some(url));
                    }
                }
            }
        }
        Message::ZoomIn => {
            b.pm().tab_mut().zoom_step(1);
            b.push_zoom();
        }
        Message::ZoomOut => {
            b.pm().tab_mut().zoom_step(-1);
            b.push_zoom();
        }
        Message::ZoomReset => {
            b.pm().tab_mut().zoom = 100;
            b.push_zoom();
        }
        Message::ExitPageFullscreen => {
            if let Some(engine) = &b.engine { engine.exit_fullscreen(b.p().tab().id); }
        }
        Message::ToggleFullscreen => {
            b.fullscreen = !b.fullscreen;
            b.popup = Popup::None;
            let mode = if b.fullscreen { window::Mode::Fullscreen } else { window::Mode::Windowed };
            return window::latest().and_then(move |id| window::set_mode(id, mode));
        }
        Message::SelectProfile(i) => {
            b.profile = i.min(b.profiles.len() - 1);
            b.popup = Popup::None;
        }
        Message::AddProfile => {
            let n = b.profiles.len();
            let color = PROFILE_COLORS[n % PROFILE_COLORS.len()];
            b.profiles.push(Profile::new(&format!("Profile {}", n + 1), color, b.next_id));
            b.next_id += 1;
            b.profile = n;
            b.popup = Popup::None;
        }
    }
    Task::none()
}

/// Bare word → search; host-ish → https://.
fn normalize(s: &str) -> String {
    let s = s.trim();
    if s.contains("://") {
        s.to_string()
    } else if !s.chars().any(char::is_whitespace)
        && (s.contains('.') || s == "localhost" || s.starts_with("localhost:") || s.starts_with('['))
    {
        let scheme = if s == "localhost" || s.starts_with("localhost:") || s.starts_with("127.") || s.starts_with('[') {
            "http"
        } else {
            "https"
        };
        format!("{scheme}://{s}")
    } else {
        let mut url = Url::parse("https://duckduckgo.com/").unwrap();
        url.query_pairs_mut().append_pair("q", s);
        url.to_string()
    }
}

fn subscription(b: &Browser) -> Subscription<Message> {
    let frames = if b.animating() { window::frames().map(Message::Frame) } else { Subscription::none() };
    let page_fullscreen = b.page_fullscreen.is_some();
    let popup_open = b.popup != Popup::None;
    let keys = event::listen_with(move |e, status, _| {
        let (down, key, modifiers) = match e {
            Event::Window(window::Event::Unfocused) => return Some(Message::ReleaseKeys),
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => (true, key, modifiers),
            Event::Keyboard(keyboard::Event::KeyReleased { key, modifiers, .. }) => (false, key, modifiers),
            _ => return None,
        };
        // a focused text input (the omnibox) captures its keys; only unclaimed ones reach the page
        let to_page = || (status == event::Status::Ignored).then(|| Message::WebKey(down, key.clone(), modifiers));
        if !down {
            return to_page();
        }
        if key.as_ref() == Key::Named(keyboard::key::Named::Escape) {
            return if page_fullscreen { Some(Message::ExitPageFullscreen) }
                else if popup_open { Some(Message::ClosePopup) } else { to_page() };
        }
        if !modifiers.command() {
            return to_page();
        }
        let shift = modifiers.shift();
        match key.as_ref() {
            Key::Character("t") if shift => Some(Message::ReopenTab),
            Key::Character("t") => Some(Message::NewTab),
            Key::Character("d") => Some(Message::Bookmark),
            Key::Character("n") if shift => Some(Message::Menu(MenuItem::Incognito)),
            Key::Character("n") => Some(Message::Menu(MenuItem::NewWindow)),
            Key::Character("w") => Some(Message::CloseActive),
            Key::Character("l") if modifiers.alt() => Some(Message::Menu(MenuItem::Downloads)),
            Key::Character("l") => Some(Message::FocusOmnibox),
            Key::Character("r") => Some(Message::Reload),
            Key::Character("[") => Some(Message::Back),
            Key::Character("]") => Some(Message::Forward),
            Key::Character("=") | Key::Character("+") => Some(Message::ZoomIn),
            Key::Character("-") => Some(Message::ZoomOut),
            Key::Character("0") => Some(Message::ZoomReset),
            Key::Character(",") => Some(Message::Menu(MenuItem::Settings)),
            Key::Character("y") => Some(Message::Menu(MenuItem::History)),
            Key::Character(c) => c
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=9).contains(n))
                .map(|n| if n == 9 { Message::SelectLastTab } else { Message::SelectTab(n - 1) })
                .or_else(to_page),
            _ => to_page(),
        }
    });
    let resize = window::resize_events().map(|(_, s)| Message::Resized(s));
    let wake = if b.engine.is_some() { engine::wakes().map(|_| Message::Wake) } else { Subscription::none() };
    Subscription::batch([keys, frames, resize, wake])
}

// ---------- palette ----------

#[derive(Clone, Copy)]
struct Palette {
    chrome: Color,
    strip: Color,
    tab_active: Color,
    tab_hover: Color,
    omnibox: Color,
    omnibox_focus: Color,
    content: Color,
    menu: Color,
    text: Color,
    muted: Color,
    divider: Color,
    accent: Color,
}

const fn c(r: u8, g: u8, b: u8) -> Color {
    Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
}

const DARK: Palette = Palette {
    chrome: c(0x1f, 0x1f, 0x22),
    strip: c(0x17, 0x17, 0x1a),
    tab_active: c(0x2a, 0x2a, 0x2e),
    tab_hover: c(0x22, 0x22, 0x26),
    omnibox: c(0x2a, 0x2a, 0x2e),
    omnibox_focus: c(0x33, 0x33, 0x38),
    content: c(0x14, 0x14, 0x16),
    menu: c(0x26, 0x26, 0x2a),
    text: c(0xe8, 0xe8, 0xea),
    muted: c(0x8f, 0x8f, 0x96),
    divider: c(0x2e, 0x2e, 0x33),
    accent: c(0x8b, 0x7c, 0xf6),
};

const LIGHT: Palette = Palette {
    chrome: c(0xf3, 0xf3, 0xf5),
    strip: c(0xe6, 0xe6, 0xea),
    tab_active: c(0xff, 0xff, 0xff),
    tab_hover: c(0xec, 0xec, 0xef),
    omnibox: c(0xe6, 0xe6, 0xea),
    omnibox_focus: c(0xff, 0xff, 0xff),
    content: c(0xff, 0xff, 0xff),
    menu: c(0xff, 0xff, 0xff),
    text: c(0x1c, 0x1c, 0x1e),
    muted: c(0x6e, 0x6e, 0x76),
    divider: c(0xd8, 0xd8, 0xdd),
    accent: c(0x6d, 0x5d, 0xe6),
};

// ---------- animation helpers ----------

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: lerp(a.r, b.r, t), g: lerp(a.g, b.g, t), b: lerp(a.b, b.b, t), a: lerp(a.a, b.a, t) }
}

fn fade(c: Color, t: f32) -> Color {
    Color { a: c.a * t, ..c }
}

impl Palette {
    fn map(self, o: Palette, f: impl Fn(Color, Color) -> Color) -> Palette {
        Palette {
            chrome: f(self.chrome, o.chrome),
            strip: f(self.strip, o.strip),
            tab_active: f(self.tab_active, o.tab_active),
            tab_hover: f(self.tab_hover, o.tab_hover),
            omnibox: f(self.omnibox, o.omnibox),
            omnibox_focus: f(self.omnibox_focus, o.omnibox_focus),
            content: f(self.content, o.content),
            menu: f(self.menu, o.menu),
            text: f(self.text, o.text),
            muted: f(self.muted, o.muted),
            divider: f(self.divider, o.divider),
            accent: f(self.accent, o.accent),
        }
    }
    fn mix(self, o: Palette, t: f32) -> Palette {
        self.map(o, |a, b| mix(a, b, t))
    }
    fn fade(self, t: f32) -> Palette {
        self.map(self, |a, _| fade(a, t))
    }
}

/// Stable hover key from a name (FNV-1a).
fn kid(s: &str) -> Key2 {
    ("hover", s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)) as usize)
}

/// Wrap `el` so entering/leaving drives its hover animation.
fn hov<'a>(key: Key2, el: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    mouse_area(el).on_enter(Message::Hover(key, true)).on_exit(Message::Hover(key, false)).into()
}

// ---------- view ----------

fn view(b: &Browser) -> Element<'_, Message> {
    if b.page_fullscreen == Some(b.p().tab().id) {
        return content(b, LIGHT.mix(DARK, b.theme_a.interpolate(0.0, 1.0, b.now)));
    }
    let n = b.now;
    let p = LIGHT.mix(DARK, b.theme_a.interpolate(0.0, 1.0, n));
    let side_w = SIDEBAR_W * b.sidebar_a.interpolate(0.0, 1.0, n);

    let mut col = column![];
    col = col.push(toolbar(b, p));
    col = col.push(container(space().width(Fill)).height(1).width(Fill).style(move |_| bg(p.divider)));
    col = col.push(if side_w > 0.5 {
        let vline = container(space().height(Fill)).width(1).height(Fill).style(move |_| bg(p.divider));
        row![container(tab_sidebar(b, p)).width(side_w).height(Fill).clip(true), vline, content(b, p)].into()
    } else {
        content(b, p)
    });

    let t = b.popup_a.interpolate(0.0, 1.0, n);
    if b.shown == Popup::None || (t <= 0.001 && !b.popup_a.value()) {
        return col.into();
    }

    // Popup layer: click-catcher underneath, menu box top-right, fades + slides down from the chrome.
    let chrome_h = TOOLBAR_BTN + 2.0 * BASE + 1.0;
    let pp = p.fade(t);
    let panel = match b.shown {
        Popup::Menu => app_menu(b, pp, t),
        Popup::Profiles => profile_menu(b, pp, t),
        Popup::None => unreachable!(),
    };
    let anchored =
        container(panel).width(Fill).height(Fill).align_x(iced::Right).align_y(iced::Top).padding(iced::Padding {
            top: chrome_h + BASE - 8.0 * (1.0 - t),
            right: OMNIBOX_MARGIN,
            bottom: 0.0,
            left: 0.0,
        });
    if b.popup == Popup::None {
        // closing: let clicks fall through while it fades
        return stack![col, anchored].into();
    }
    let catcher = mouse_area(container(space()).width(Fill).height(Fill)).on_press(Message::ClosePopup);
    stack![col, catcher, anchored].into()
}

fn tab_sidebar(b: &Browser, p: Palette) -> Element<'_, Message> {
    let prof = b.p();
    let new_tab = menu_row(b, kid("side:new"), "plus", "New Tab", "", p, Message::NewTab);
    let mut tabs = column![].spacing(BASE);
    for (i, t) in prof.tabs.iter().enumerate() {
        let h = TAB_H * b.t(("appear", t.id));
        tabs = tabs.push(container(tab_item(b, i, t, p)).height(h).clip(true));
    }
    let tabs = tabs.push(new_tab);
    container(scrollable(tabs).height(Fill))
        .padding(BASE)
        .width(SIDEBAR_W)
        .height(Fill)
        .style(move |_| bg(p.strip))
        .into()
}

/// One tab pill in the sidebar.
fn tab_item<'a>(b: &Browser, i: usize, t: &'a Tab, p: Palette) -> Element<'a, Message> {
    let hk = kid(&format!("tab{}", t.id));
    let xk = kid(&format!("x{}", t.id));
    let (a, h, x, ap) = (b.t(("active", t.id)), b.t(hk), b.t(xk), b.t(("appear", t.id)));
    let p = p.fade(ap);

    // close button fades in on hover / active
    let close = button(icon("x", 13.0, fade(mix(p.muted, p.text, x), a.max(h))))
        .padding([0, 4])
        .on_press(Message::CloseTab(t.id))
        .style(move |_, s| flat(p, x, s));
    let label = text(if t.loading { format!("↻ {}", t.title) } else { t.title.clone() })
        .size(12.5)
        .color(mix(p.muted, p.text, a.max(h * 0.6)))
        .wrapping(text::Wrapping::None);
    let mask = t.incognito.then(|| icon("venetian-mask", 13.0, mix(p.muted, p.text, a.max(h * 0.6))));
    let inner = row![mask, label, space().width(Fill), hov(xk, close)].align_y(iced::Center).spacing(BASE);
    let fill = mix(mix(p.strip, p.tab_hover, h), p.tab_active, a);
    mouse_area(
        container(inner)
            .padding([0.0, 8.0 + 2.0 * h * (1.0 - a)])
            .height(TAB_H)
            .width(Fill)
            .align_y(iced::Center)
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: Border { radius: TAB_RADIUS.into(), ..Default::default() },
                ..Default::default()
            }),
    )
    .on_press(Message::SelectTab(i))
    .on_enter(Message::Hover(hk, true))
    .on_exit(Message::Hover(hk, false))
    .into()
}

fn toolbar(b: &Browser, p: Palette) -> Element<'_, Message> {
    let prof = b.p();
    let t = prof.tab();
    let nav = b.can_nav();
    let ok = kid("omnibox");
    let h = b.t(ok);
    let omnibox = text_input("Search or enter address", &t.input)
        .id(OMNIBOX_ID)
        .on_input(Message::UrlInput)
        .on_submit(Message::UrlSubmit)
        .padding([0, 10])
        .size(13)
        .line_height(text::LineHeight::Absolute(OMNIBOX_H.into()))
        .style(move |_, s| {
            let focused = matches!(s, text_input::Status::Focused { .. });
            text_input::Style {
                background: if focused { p.omnibox_focus } else { mix(p.omnibox, p.omnibox_focus, h * 0.5) }.into(),
                border: Border {
                    radius: OMNIBOX_RADIUS.into(),
                    width: 1.0,
                    color: if focused { p.accent } else { fade(p.divider, h) },
                },
                icon: p.muted,
                placeholder: mix(p.muted, p.text, h * 0.3),
                value: p.text,
                selection: Color { a: 0.35, ..p.accent },
            }
        });

    let bar = row![
        icon_btn(b, "chevron-left", p, nav.0.then_some(Message::Back)),
        icon_btn(b, "chevron-right", p, nav.1.then_some(Message::Forward)),
        icon_btn(b, "rotate-cw", p, Some(Message::Reload)),
        container(hov(ok, omnibox)).padding([0.0, OMNIBOX_MARGIN]).width(Fill),
        icon_btn(b, "star", p, Some(Message::Bookmark)),
        icon_btn(b, "sun-moon", p, Some(Message::ToggleTheme)),
        icon_btn(b, "panel-left", p, Some(Message::ToggleSidebar)),
        avatar_btn(b, prof, p),
        icon_btn(b, "ellipsis-vertical", p, Some(Message::TogglePopup(Popup::Menu))),
    ]
    .spacing(BASE)
    .align_y(iced::Center);

    // macOS: clear the traffic lights (hidden in fullscreen)
    let left = if cfg!(target_os = "macos") && !b.fullscreen { TRAFFIC_LIGHTS_W } else { BASE };
    let bar = container(bar)
        .padding(iced::Padding { left, ..iced::Padding::new(BASE) })
        .width(Fill)
        .style(move |_| bg(p.chrome));
    // empty toolbar space drags / zooms the window (buttons + omnibox capture their own clicks)
    mouse_area(bar)
        .on_press(Message::ArmDrag(true))
        .on_release(Message::ArmDrag(false))
        .on_move(|_| Message::DragWindow)
        .on_double_click(Message::ZoomWindow)
        .into()
}

fn avatar_btn<'a>(b: &Browser, prof: &Profile, p: Palette) -> Element<'a, Message> {
    let k = kid("avatar");
    let h = b.t(k);
    let initial = prof.name.chars().next().unwrap_or('?').to_ascii_uppercase().to_string();
    let color = prof.color;
    // ring grows on hover
    let size = 20.0 + 2.0 * h;
    let circle = container(text(initial).size(11).color(Color::WHITE)).center(size).style(move |_| container::Style {
        background: Some(color.into()),
        border: Border { radius: (size / 2.0).into(), width: 2.0 * h, color: fade(Color::WHITE, 0.35 * h) },
        ..Default::default()
    });
    hov(
        k,
        button(container(circle).center(TOOLBAR_BTN))
            .padding(0)
            .on_press(Message::TogglePopup(Popup::Profiles))
            .style(move |_, s| flat(p, h, s)),
    )
}

fn content(b: &Browser, p: Palette) -> Element<'_, Message> {
    let t = b.p().tab();
    if let Some(error) = &t.error {
        return container(
            column![
                text("Page unavailable").size(28),
                text(error).size(13),
                button("Reload page").on_press(Message::Reload),
            ]
            .spacing(12),
        )
        .center(Fill)
        .into();
    }
    if t.url == "bro://private-browsing" {
        return container(
            column![
                text("Private browsing unavailable").size(28),
                text("This build shares Servo cookies and storage across tabs and profiles."),
                text("Private browsing needs a separate engine storage context."),
            ]
            .spacing(12),
        )
        .padding(32)
        .width(Fill)
        .height(Fill)
        .into();
    }
    if t.url == "bro://bookmarks" {
        let mut list =
            column![text("Bookmarks").size(28), text("Session bookmarks · ⌘D toggles current page")].spacing(12);
        for (title, url) in &b.p().bookmarks {
            list = list.push(button(text(format!("{title} — {url}"))).on_press(Message::Navigate(url.clone())));
        }
        if b.p().bookmarks.is_empty() {
            list = list.push(text("No bookmarks yet."));
        }
        return container(scrollable(list)).padding(32).width(Fill).height(Fill).into();
    }
    if t.url == "bro://settings" || t.url == "bro://customize" {
        return container(
            column![
                text("Settings").size(28),
                button(if b.dark { "Use light theme" } else { "Use dark theme" }).on_press(Message::ToggleTheme),
                button(if b.sidebar { "Hide sidebar" } else { "Show sidebar" }).on_press(Message::ToggleSidebar),
                text("Bookmarks and closed tabs stay in memory for this session."),
                text("Profiles separate tab sessions only; cookies and storage are shared."),
            ]
            .spacing(16),
        )
        .padding(32)
        .width(Fill)
        .height(Fill)
        .into();
    }
    if let Some(frame) = b.engine.as_ref().and_then(|e| e.frame(t.id)) {
        // The frame is exactly content_size() device pixels and the widget fills that same logical box → 1:1.
        let page = image(frame)
            .width(Fill)
            .height(Fill)
            .content_fit(iced::ContentFit::Fill)
            .filter_method(image::FilterMethod::Nearest);
        return mouse_area(page)
            .on_move(Message::WebMove)
            .on_press(Message::WebButton(engine::Button::Left, true))
            .on_release(Message::WebButton(engine::Button::Left, false))
            .on_right_press(Message::WebButton(engine::Button::Right, true))
            .on_right_release(Message::WebButton(engine::Button::Right, false))
            .on_middle_press(Message::WebButton(engine::Button::Middle, true))
            .on_middle_release(Message::WebButton(engine::Button::Middle, false))
            .on_scroll(Message::WebScroll)
            .into();
    }
    let a = b.content_a.interpolate(0.0, 1.0, b.now);
    let loading = b.engine.as_ref().is_some_and(|e| e.has(t.id));
    let (h1, sub): (String, String) = if t.url.is_empty() {
        ("bro".into(), "new tab".into())
    } else if t.url.starts_with("bro://") {
        (t.title.clone(), format!("{} · internal page, not built yet", t.url))
    } else if loading {
        (t.title.clone(), "loading…".into())
    } else {
        (t.url.clone(), "engine not available for this URL".into())
    };
    let body = column![text(h1).size(48).color(fade(p.text, a)), text(sub).size(13).color(fade(p.muted, a))]
        .spacing(8)
        .align_x(iced::Center);
    // fade in + rise 10px
    container(body)
        .width(Fill)
        .height(Fill)
        .center(Fill)
        .padding(iced::Padding { top: 20.0 * (1.0 - a), ..Default::default() })
        .style(move |_| bg(p.content))
        .into()
}

// ---------- popups ----------

fn app_menu(b: &Browser, p: Palette, t: f32) -> Element<'_, Message> {
    let mut col = column![].spacing(0);
    for item in MenuItem::ALL {
        let el: Element<'_, Message> = match item {
            MenuItem::Divider1 | MenuItem::Divider2 | MenuItem::Divider3 | MenuItem::Divider4 => divider(p),
            MenuItem::Zoom => zoom_row(b, p),
            _ => {
                let (glyph, label, shortcut, sub) = item.spec();
                let trailing = if sub { "›" } else { shortcut };
                menu_row(b, kid(label), glyph, label, trailing, p, Message::Menu(item))
            }
        };
        col = col.push(el);
    }
    panel(col.into(), p, t)
}

fn profile_menu(b: &Browser, p: Palette, t: f32) -> Element<'_, Message> {
    let mut col = column![container(text("Profiles").size(11).color(p.muted)).padding([6, 12]),];
    for (i, prof) in b.profiles.iter().enumerate() {
        let k = kid(&format!("prof{i}"));
        let h = b.t(k);
        let active = i == b.profile;
        let color = fade(prof.color, p.text.a);
        let d = 10.0 + 2.0 * h;
        let dot = container(space()).center(d).style(move |_| container::Style {
            background: Some(color.into()),
            border: Border { radius: (d / 2.0).into(), ..Default::default() },
            ..Default::default()
        });
        let label = text(&prof.name).size(13).color(p.text);
        let check = active.then(|| icon("check", 14.0, p.accent));
        let inner = row![dot, label, space().width(Fill), check].spacing(10).align_y(iced::Center);
        col = col.push(hov(
            k,
            button(inner)
                .width(Fill)
                .padding([6.0, 12.0 + 3.0 * h])
                .on_press(Message::SelectProfile(i))
                .style(move |_, s| flat(p, h, s)),
        ));
    }
    col = col.push(divider(p));
    col = col.push(menu_row(b, kid("add profile"), "plus", "Add profile", "", p, Message::AddProfile));
    col = col.push(menu_row(
        b,
        kid("manage profiles"),
        "settings",
        "Manage profiles",
        "›",
        p,
        Message::Menu(MenuItem::Settings),
    ));
    panel(col.into(), p, t)
}

fn panel<'a>(body: Element<'a, Message>, p: Palette, t: f32) -> Element<'a, Message> {
    container(body)
        .width(MENU_W)
        .padding(4)
        .style(move |_| container::Style {
            background: Some(p.menu.into()),
            border: Border { radius: MENU_RADIUS.into(), width: 1.0, color: p.divider },
            shadow: Shadow {
                color: Color { a: 0.45 * t, ..Color::BLACK },
                offset: Vector::new(0.0, 6.0 * t),
                blur_radius: 18.0 * t,
            },
            ..Default::default()
        })
        .into()
}

fn menu_row<'a>(
    b: &Browser,
    k: Key2,
    glyph: &'static str,
    label: &'a str,
    trailing: &'a str,
    p: Palette,
    on: Message,
) -> Element<'a, Message> {
    let h = b.t(k);
    let inner = row![
        container(icon(glyph, 16.0, mix(p.muted, p.accent, h))).center(18),
        text(label).size(13).color(p.text),
        space().width(Fill),
        if trailing == "›" {
            icon("chevron-right", 13.0, mix(p.muted, p.text, h))
        } else {
            text(trailing).size(12).color(mix(p.muted, p.text, h)).into()
        },
    ]
    .spacing(10)
    .align_y(iced::Center);
    // label nudges right on hover
    let pad = iced::Padding { top: 6.0, bottom: 6.0, left: 10.0 + 3.0 * h, right: 10.0 };
    hov(k, button(inner).width(Fill).padding(pad).on_press(on).style(move |_, s| flat(p, h, s)))
}

fn zoom_row(b: &Browser, p: Palette) -> Element<'_, Message> {
    let zoom = b.p().tab().zoom;
    let small = |g: &'static str, m: Message| {
        let k = kid(&format!("zoom{g}"));
        let h = b.t(k);
        hov(
            k,
            button(container(icon(g, 14.0, p.text)).center(24)).padding(0).on_press(m).style(move |_, s| flat(p, h, s)),
        )
    };
    let rk = kid("zoom%");
    let rh = b.t(rk);
    let inner = row![
        container(icon("zoom-in", 16.0, p.muted)).center(18),
        text("Zoom").size(13).color(p.text),
        space().width(Fill),
        small("minus", Message::ZoomOut),
        hov(
            rk,
            button(text(format!("{zoom}%")).size(12).color(p.text))
                .padding([0, 6])
                .on_press(Message::ZoomReset)
                .style(move |_, s| flat(p, rh, s)),
        ),
        small("plus", Message::ZoomIn),
        space().width(8),
        small("maximize", Message::ToggleFullscreen),
    ]
    .spacing(6)
    .align_y(iced::Center);
    container(inner).width(Fill).padding([4, 10]).into()
}

fn divider(p: Palette) -> Element<'static, Message> {
    container(container(space().width(Fill)).height(1).width(Fill).style(move |_| bg(p.divider)))
        .padding([4, 0])
        .width(Fill)
        .into()
}

// ---------- shared styles ----------

fn icon_btn<'a>(b: &Browser, glyph: &'static str, p: Palette, on: Option<Message>) -> Element<'a, Message> {
    let k = kid(glyph);
    let h = b.t(k);
    // back/forward fade between enabled and disabled
    let en = match glyph {
        "chevron-left" => b.t(("en", 0)),
        "chevron-right" => b.t(("en", 1)),
        _ => 1.0,
    };
    let color = if glyph == "star" && b.p().bookmarks.iter().any(|(_, url)| url == &b.p().tab().url) {
        p.accent
    } else {
        mix(p.muted, p.text, en)
    };
    let btn = button(container(icon(glyph, 16.0, color)).center(TOOLBAR_BTN))
        .padding(0)
        .style(move |_, s| flat(p, h * en, s));
    let Some(m) = on else {
        // disabled buttons don't capture clicks; swallow them so they don't drag/zoom the window
        return hov(k, mouse_area(btn).on_press(Message::Noop));
    };
    hov(k, btn.on_press(m))
}

/// Lucide icon (ISC, assets/icons), tinted to `color`.
fn icon<'a>(name: &str, size: f32, color: Color) -> Element<'a, Message> {
    macro_rules! icons {
        ($($n:literal),*) => {
            match name {
                $($n => include_bytes!(concat!("../assets/icons/", $n, ".svg")).as_slice(),)*
                _ => unreachable!("unknown icon {name}"),
            }
        };
    }
    let bytes = icons!(
        "app-window",
        "check",
        "chevron-left",
        "chevron-right",
        "clock-arrow-left",
        "download",
        "ellipsis-vertical",
        "info",
        "layout-grid",
        "maximize",
        "minus",
        "paintbrush",
        "panel-left",
        "plus",
        "printer",
        "puzzle",
        "rotate-cw",
        "search",
        "settings",
        "share",
        "square-plus",
        "star",
        "sun-moon",
        "trash",
        "venetian-mask",
        "wrench",
        "x",
        "zoom-in"
    );
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        // svg tint replaces rgb only and drops alpha, so fades go through opacity
        .style(move |_, _| svg::Style { color: Some(Color { a: 1.0, ..color }) })
        .opacity(color.a)
        .into()
}

/// Flat button: hover background fades in with `h` (0..1); press is instant.
fn flat(p: Palette, h: f32, s: button::Status) -> button::Style {
    let background = match s {
        button::Status::Pressed => p.tab_active,
        _ => fade(p.tab_hover, h),
    };
    button::Style {
        background: Some(background.into()),
        text_color: p.text,
        border: Border { radius: TAB_RADIUS.into(), ..Default::default() },
        ..Default::default()
    }
}

fn bg(color: Color) -> container::Style {
    container::Style { background: Some(color.into()), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ninth_tab_click_and_last_tab_shortcut_differ() {
        let mut b = Browser::default();
        for _ in 0..11 {
            let _ = apply(&mut b, Message::NewTab);
        }
        let _ = apply(&mut b, Message::SelectTab(8));
        assert_eq!(b.p().active, 8);
        let _ = apply(&mut b, Message::SelectLastTab);
        assert_eq!(b.p().active, 11);
    }

    #[test]
    fn searches_preserve_query_characters() {
        for query in ["a & b", "rust #traits", "x+y = z", "한글 검색", "a\tb"] {
            let url = Url::parse(&normalize(query)).unwrap();
            assert_eq!(url.query_pairs().collect::<Vec<_>>(), vec![("q".into(), query.into())]);
            assert_eq!(url.fragment(), None);
        }
        assert_eq!(normalize("localhost:8000"), "http://localhost:8000");
        assert_eq!(normalize("127.0.0.1:3000"), "http://127.0.0.1:3000");
        assert_eq!(normalize("[::1]:8080"), "http://[::1]:8080");
    }

    #[test]
    fn closed_tabs_restore_with_fresh_ids_and_keep_profile_scope() {
        let mut b = Browser::default();
        let _ = update(&mut b, Message::NewTab);
        b.pm().tab_mut().goto("bro://settings".into());
        let _ = update(&mut b, Message::CloseActive);
        assert_eq!(b.p().closed.len(), 1);
        let _ = update(&mut b, Message::AddProfile);
        let _ = update(&mut b, Message::ReopenTab);
        assert_eq!(b.p().tabs.len(), 1);
        let _ = update(&mut b, Message::SelectProfile(0));
        let _ = update(&mut b, Message::ReopenTab);
        assert_eq!(b.p().tab().url, "bro://settings");
        assert_ne!(b.p().tab().id, 1);
        assert!(b.p().closed.is_empty());
    }

    #[test]
    fn canonical_url_does_not_truncate_restored_forward_history() {
        let mut t = Tab::new(0);
        t.goto("https://example.com".into());
        t.goto("https://example.org".into());
        t.cursor = 0;
        t.set_current(t.history[0].clone());
        t.shell_history = true;
        t.input = "unfinished search".into();
        t.receive_url(Url::parse("https://example.com/").unwrap());
        assert!(t.can_forward());
        assert_eq!(t.history.len(), 2);
        assert_eq!(t.input, "unfinished search");
    }

    #[test]
    fn reopened_tabs_keep_history_navigation() {
        let mut b = Browser::default();
        let _ = apply(&mut b, Message::NewTab);
        b.pm().tab_mut().goto("bro://settings".into());
        b.pm().tab_mut().goto("bro://bookmarks".into());
        let _ = apply(&mut b, Message::CloseActive);
        let _ = apply(&mut b, Message::ReopenTab);
        assert!(b.p().tab().shell_history);
        assert_eq!(b.can_nav(), (true, false));
        let _ = apply(&mut b, Message::Back);
        assert_eq!(b.p().tab().url, "bro://settings");
        assert_eq!(b.can_nav(), (false, true));
        let _ = apply(&mut b, Message::Forward);
        assert_eq!(b.p().tab().url, "bro://bookmarks");
    }

    #[test]
    fn bookmarks_toggle_and_stay_in_profile() {
        let mut b = Browser::default();
        b.pm().tab_mut().goto("https://example.com/".into());
        let _ = apply(&mut b, Message::Bookmark);
        assert_eq!(b.p().bookmarks.len(), 1);
        let _ = apply(&mut b, Message::Bookmark);
        assert!(b.p().bookmarks.is_empty());
        let _ = apply(&mut b, Message::Bookmark);
        let _ = apply(&mut b, Message::AddProfile);
        assert!(b.p().bookmarks.is_empty());
    }

    #[test]
    fn private_browsing_does_not_claim_storage_isolation() {
        let mut b = Browser::default();
        let _ = update(&mut b, Message::Menu(MenuItem::Incognito));
        assert_eq!(b.p().tab().url, "bro://private-browsing");
        assert!(!b.p().tab().incognito);
    }

    #[test]
    fn normalize_rules() {
        assert_eq!(normalize("https://a.b"), "https://a.b");
        assert_eq!(normalize("example.com"), "https://example.com");
        assert_eq!(normalize("rust lang"), "https://duckduckgo.com/?q=rust+lang");
        assert_eq!(page_title("bro://tab-groups"), "Tab groups");
    }

    #[test]
    fn history_back_forward_truncates() {
        let mut t = Tab::new(0);
        t.goto("a".into());
        t.goto("b".into());
        t.goto("c".into());
        assert!(t.can_back() && !t.can_forward());
        t.cursor -= 1;
        t.cursor -= 1;
        t.goto("d".into());
        assert_eq!(t.history, vec!["a", "d"]);
        assert!(!t.can_forward());
    }

    #[test]
    fn close_tab_keeps_active_sane() {
        let mut b = Browser::default();
        let _ = update(&mut b, Message::NewTab);
        let _ = update(&mut b, Message::NewTab);
        b.pm().active = 2;
        let _ = update(&mut b, Message::CloseTab(0));
        assert_eq!((b.p().tabs.len(), b.p().active), (2, 1));
        let _ = update(&mut b, Message::CloseActive);
        assert_eq!((b.p().tabs.len(), b.p().active), (1, 0));
    }

    #[test]
    fn profiles_own_their_tabs() {
        let mut b = Browser::default();
        let _ = update(&mut b, Message::NewTab);
        let _ = update(&mut b, Message::AddProfile);
        assert_eq!((b.profile, b.p().tabs.len()), (1, 1));
        let _ = update(&mut b, Message::Menu(MenuItem::Settings));
        assert_eq!(b.p().tab().url, "bro://settings");
        let _ = update(&mut b, Message::SelectProfile(0));
        assert_eq!(b.p().tabs.len(), 2);
    }

    #[test]
    fn zoom_steps_clamp() {
        let mut t = Tab::new(0);
        for _ in 0..30 {
            t.zoom_step(1);
        }
        assert_eq!(t.zoom, 500);
        for _ in 0..30 {
            t.zoom_step(-1);
        }
        assert_eq!(t.zoom, 25);
    }

    #[test]
    fn sidebar_toggles_visibility() {
        let mut b = Browser::default();
        assert!(b.sidebar);
        let _ = update(&mut b, Message::ToggleSidebar);
        assert!(!b.sidebar && !b.sidebar_a.value());
        let _ = update(&mut b, Message::ToggleSidebar);
        assert!(b.sidebar && b.sidebar_a.value());
    }

    #[test]
    fn animations_track_state() {
        let mut b = Browser::default();
        assert!(!b.animating());
        let _ = update(&mut b, Message::ToggleTheme);
        assert!(b.animating() && !b.theme_a.value());
        let _ = update(&mut b, Message::TogglePopup(Popup::Menu));
        let _ = update(&mut b, Message::ClosePopup);
        // panel still drawn while it fades out
        assert_eq!((b.popup, b.shown, b.popup_a.value()), (Popup::None, Popup::Menu, false));
        let _ = update(&mut b, Message::NewTab);
        assert!(b.t(("appear", 1)) < 1.0 && b.anims[&("active", 1)].value() && !b.anims[&("active", 0)].value());
        let later = b.now + std::time::Duration::from_secs(2);
        let _ = update(&mut b, Message::Frame(later));
        assert!(!b.animating() && !b.anims.contains_key(&("appear", 1)));
    }
}
