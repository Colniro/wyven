// Release builds on Windows open no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Wyvern – the editor app: window, input, and wiring of tabs, explorer, highlighting and
//! renderer.
//!
//! Start: `wyvern <ordner>` opens a project, `wyvern <datei>` opens the file and its folder.
//! Without an argument the most recently used project opens.

mod completion;
mod config;
mod editor;
mod explorer;
mod index;
mod keymap;
mod leader;
mod lsp;
mod metrics;
mod palette;
mod popup;
mod theme;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};
use wyvern_buffer::{Document, Movement};
use wyvern_render::{
    measure_cell_width, pick_monospace_family, Attrs, Buffer, Family, FontSystem, Metrics, Quad,
    Renderer, Rgba, Scene, Shaping, TextBlock, Wrap,
};

use editor::{EditorView, FontMetrics, Rect};
use explorer::Explorer;
use index::FileIndex;
use keymap::{Command, Keymap};
use metrics::InputLatency;
use palette::Palette;
use popup::Popup;
use theme::Theme;

const DEFAULT_FONT_SIZE: f32 = 14.0;
/// Panels (explorer, tabs, status bar) use a slightly smaller font than the code.
const UI_FONT_SCALE: f32 = 0.93;
const EXPLORER_CELLS: f32 = 30.0;
const STATS_INTERVAL: Duration = Duration::from_millis(500);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const MESSAGE_TIME: Duration = Duration::from_secs(4);
const LEADER_POPUP_DELAY: Duration = Duration::from_millis(300);
const CONFIRM_TIME: Duration = Duration::from_secs(3);
const HOVER_DELAY: Duration = Duration::from_millis(600);

/// Wakes the event loop from other threads (language server messages).
#[derive(Debug, Clone, Copy)]
enum UserEvent {
    Lsp,
}

/// Startup profiling: logs elapsed time and RAM after each startup stage.
fn log_stage(stage: &str, started: Instant) {
    let ram = metrics::ram_mb().map_or("–".into(), |mb| format!("{mb:.0} MB"));
    log::info!("startup {stage}: {} · RAM {ram}", metrics::ms(Some(started.elapsed())));
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App { path, state: None, proxy };
    event_loop.run_app(&mut app).expect("event loop error");
}

struct App {
    path: Option<PathBuf>,
    state: Option<State>,
    proxy: EventLoopProxy<UserEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Editor,
    Explorer,
}

enum Overlay {
    None,
    Leader { group: &'static str, since: Instant, shown: bool },
    Recent(Vec<PathBuf>),
    Palette,
}

struct Mouse {
    x: f32,
    y: f32,
    dragging: bool,
    last_click: Option<(Instant, usize)>,
}

/// Screen layout, recomputed on resize, font change and when the explorer is toggled.
#[derive(Default, Clone, Copy)]
struct Layout {
    explorer: Option<Rect>,
    tabs: Rect,
    editor: Rect,
    status: Rect,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
    theme: Theme,
    family: String,
    font_pt: f32,
    font: FontMetrics,
    ui_font: FontMetrics,
    layout: Layout,

    tabs: Vec<EditorView>,
    active: usize,
    /// Tab indices, most recently used first.
    mru: Vec<usize>,
    /// Position in `mru` while Strg is held and Tab pressed repeatedly.
    cycling: Option<usize>,
    closed: Vec<PathBuf>,
    tab_bar: Buffer,
    tab_spans: Vec<(f32, f32)>,

    explorer: Option<Explorer>,
    explorer_visible: bool,
    focus: Focus,

    overlay: Overlay,
    popup: Popup,
    palette: Palette,
    index: FileIndex,
    keymap: Keymap,
    modifiers: ModifiersState,
    alt_alone: bool,
    mouse: Mouse,
    clipboard: Option<arboard::Clipboard>,
    last_copy: Option<(String, bool)>,

    status_left: Buffer,
    status_right: Buffer,
    message: Option<(String, Instant)>,
    /// A destructive action waiting for confirmation by repeating it (close dirty tab/window).
    armed: Option<(&'static str, usize, Instant)>,

    proxy: EventLoopProxy<UserEvent>,
    lsp: Option<lsp::LanguageServer>,
    lsp_failed: bool,
    completion: Option<completion::Completion>,
    tooltip: Option<completion::Tooltip>,
    /// Where and since when the mouse rests (for hover info).
    hover_rest: Option<(Instant, f32, f32)>,

    latency: InputLatency,
    started: Instant,
    first_frame: Option<Duration>,
    next_stats: Instant,
    title: String,
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let started = Instant::now();
        // 1200×800, but never larger than 85 % of the screen (title bar and taskbar need room).
        // Centered on the primary screen: Windows otherwise cascades every new window further
        // down until the status bar ends up below the screen edge.
        let (mut w, mut h) = (1200.0, 800.0);
        let mut position = None;
        if let Some(m) = event_loop.primary_monitor() {
            let scale = m.scale_factor();
            let size = m.size().to_logical::<f64>(scale);
            let origin = m.position().to_logical::<f64>(scale);
            w = f64::min(w, size.width * 0.85);
            h = f64::min(h, size.height * 0.85);
            position = Some(winit::dpi::LogicalPosition::new(
                origin.x + (size.width - w) / 2.0,
                origin.y + (size.height - h) / 2.0 - 20.0,
            ));
        }
        // Created hidden and shown after the first frame: no white flash while the GPU starts.
        let mut attrs = Window::default_attributes()
            .with_title("Wyvern")
            .with_visible(false)
            .with_inner_size(LogicalSize::new(w, h));
        if let Some(p) = position {
            attrs = attrs.with_position(p);
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("failed to create window"));
        log_stage("window", started);
        let renderer =
            pollster::block_on(Renderer::new(window.clone(), event_loop.owned_display_handle()));
        log_stage("renderer", started);

        let mut state = State::new(window, renderer, started, self.proxy.clone());
        match self.path.take() {
            Some(p) if p.is_dir() => state.open_folder(&p),
            Some(p) => {
                let p = std::path::absolute(&p).unwrap_or(p);
                if let Some(parent) = p.parent() {
                    state.open_folder(parent);
                }
                state.open_path(&p);
            }
            None => {
                if let Some(last) = config::recent_projects().into_iter().next() {
                    state.open_folder(&last);
                }
            }
        }
        log_stage("state", started);
        // Hidden windows get no redraw events, so draw the first frame now; it shows the window.
        state.redraw();
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => {
                if state.confirm_close_window() {
                    if let Some(server) = &mut state.lsp {
                        server.shutdown();
                    }
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                log::debug!("resized to {}x{}", size.width, size.height);
                state.renderer.resize(size.width, size.height);
                state.relayout();
            }
            WindowEvent::ScaleFactorChanged { .. } => state.apply_font(),
            WindowEvent::Focused(true) => {
                if let Some(ex) = &mut state.explorer {
                    ex.refresh();
                    state.index.rebuild(ex.root());
                }
                state.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => state.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => state.key_event(&event),
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x as f32, position.y as f32);
                if let Some(t) = &state.tooltip
                    && ((t.x - x).abs() > 12.0 || (t.y - y).abs() > 12.0) {
                        state.tooltip = None;
                        state.window.request_redraw();
                    }
                state.mouse.x = x;
                state.mouse.y = y;
                state.hover_rest = (!state.mouse.dragging).then(|| (Instant::now(), x, y));
                if state.mouse.dragging {
                    let ed = state.editor_mut();
                    let pos = ed.hit_test(position.x as f32, position.y as f32);
                    ed.doc.extend_primary_to(pos);
                    ed.ensure_cursor_visible();
                    state.window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state: btn, button, .. } => {
                state.mouse_button(button, btn == ElementState::Pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => state.wheel(delta),
            WindowEvent::RedrawRequested => state.redraw(),
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        if let Some(state) = &mut self.state {
            state.lsp_poll();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Sleep until the next status refresh or until the leader popup is due.
        let Some(state) = &mut self.state else { return };
        let now = Instant::now();
        if now >= state.next_stats {
            state.next_stats = now + STATS_INTERVAL;
            if state.message.as_ref().is_some_and(|(_, t)| t.elapsed() > MESSAGE_TIME) {
                state.message = None;
            }
            state.window.request_redraw();
        }
        let mut wake = state.next_stats;
        if let Some(t) = state.lsp_tick(now) {
            wake = wake.min(t);
        }
        if matches!(state.overlay, Overlay::Palette) && state.palette.waiting_for_index {
            if state.index.files().is_some() {
                state.refresh_palette();
                state.window.request_redraw();
            } else {
                wake = wake.min(now + Duration::from_millis(30));
            }
        }
        if let Overlay::Leader { since, shown, .. } = &mut state.overlay {
            let due = *since + LEADER_POPUP_DELAY;
            if !*shown {
                if now >= due {
                    *shown = true;
                    state.window.request_redraw();
                } else {
                    wake = wake.min(due);
                }
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}

fn font_metrics(fs: &mut FontSystem, family: &str, size: f32) -> FontMetrics {
    let line_h = (size * 1.5).round();
    let attrs = Attrs::new().family(Family::Name(family));
    let cell_w = measure_cell_width(fs, Metrics::new(size, line_h), &attrs);
    FontMetrics { size, line_h, cell_w }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl State {
    fn new(
        window: Arc<Window>,
        mut renderer: Renderer,
        started: Instant,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
        let scale = window.scale_factor() as f32;
        let fs = renderer.font_system();
        let family = pick_monospace_family(fs).unwrap_or_else(|| "monospace".into());
        log::info!("font: {family}");
        let font = font_metrics(fs, &family, DEFAULT_FONT_SIZE * scale);
        let ui_font = font_metrics(fs, &family, DEFAULT_FONT_SIZE * scale * UI_FONT_SCALE);
        let editor = EditorView::new(Document::default(), fs, font, family.clone());
        let ui_metrics = Metrics::new(ui_font.size, ui_font.line_h);
        let mut line_buffer = || {
            let mut b = Buffer::new(fs, ui_metrics);
            b.set_wrap(Wrap::None);
            b
        };
        let (status_left, status_right, tab_bar) = (line_buffer(), line_buffer(), line_buffer());
        let popup = Popup::new(fs, ui_font);
        let palette = Palette::new(fs, ui_font);

        let mut state = Self {
            window,
            renderer,
            theme: Theme::dark(),
            family,
            font_pt: DEFAULT_FONT_SIZE,
            font,
            ui_font,
            layout: Layout::default(),
            tabs: vec![editor],
            active: 0,
            mru: vec![0],
            cycling: None,
            closed: Vec::new(),
            tab_bar,
            tab_spans: Vec::new(),
            explorer: None,
            explorer_visible: true,
            focus: Focus::Editor,
            overlay: Overlay::None,
            popup,
            palette,
            index: FileIndex::default(),
            keymap: Keymap::default(),
            modifiers: ModifiersState::empty(),
            alt_alone: false,
            mouse: Mouse { x: 0.0, y: 0.0, dragging: false, last_click: None },
            clipboard: arboard::Clipboard::new().ok(),
            last_copy: None,
            status_left,
            status_right,
            message: None,
            armed: None,
            proxy,
            lsp: None,
            lsp_failed: false,
            completion: None,
            tooltip: None,
            hover_rest: None,
            latency: InputLatency::default(),
            started,
            first_frame: None,
            next_stats: Instant::now(),
            title: String::new(),
        };
        state.relayout();
        state
    }

    fn editor(&self) -> &EditorView {
        &self.tabs[self.active]
    }

    fn editor_mut(&mut self) -> &mut EditorView {
        &mut self.tabs[self.active]
    }

    // ----- layout -----

    fn relayout(&mut self) {
        let (w, h) = self.renderer.size();
        let (w, h) = (w as f32, h as f32);
        let status_h = (self.ui_font.line_h * 1.25).round();
        let tabs_h = (self.ui_font.line_h * 1.6).round();
        let explorer_w = if self.explorer.is_some() && self.explorer_visible {
            (EXPLORER_CELLS * self.ui_font.cell_w).min(w * 0.4).round()
        } else {
            0.0
        };
        let body_h = h - status_h;
        self.layout = Layout {
            explorer: (explorer_w > 0.0).then_some(Rect { x: 0.0, y: 0.0, w: explorer_w, h: body_h }),
            tabs: Rect { x: explorer_w, y: 0.0, w: w - explorer_w, h: tabs_h },
            editor: Rect { x: explorer_w, y: tabs_h, w: w - explorer_w, h: body_h - tabs_h },
            status: Rect { x: 0.0, y: body_h, w, h: status_h },
        };
        let area = self.layout.editor;
        for t in &mut self.tabs {
            t.set_area(area);
        }
        if let (Some(ex), Some(r)) = (&mut self.explorer, self.layout.explorer) {
            ex.set_area(r);
        }
        self.window.request_redraw();
    }

    /// Re-measures the fonts (zoom or DPI change).
    fn apply_font(&mut self) {
        let scale = self.renderer.scale_factor();
        let family = self.family.clone();
        let fs = self.renderer.font_system();
        self.font = font_metrics(fs, &family, self.font_pt * scale);
        self.ui_font = font_metrics(fs, &family, self.font_pt * scale * UI_FONT_SCALE);
        let (font, ui) = (self.font, self.ui_font);
        for t in &mut self.tabs {
            t.set_font(self.renderer.font_system(), font);
        }
        if let Some(ex) = &mut self.explorer {
            ex.set_font(ui);
        }
        self.popup.set_font(ui);
        self.palette.set_font(ui);
        let m = Metrics::new(ui.size, ui.line_h);
        for b in [&mut self.status_left, &mut self.status_right, &mut self.tab_bar] {
            b.set_metrics(m);
        }
        self.relayout();
    }

    fn show(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    /// Returns true if `what` was armed for `index` within the last seconds; otherwise arms it.
    fn confirm(&mut self, what: &'static str, index: usize, msg: &str) -> bool {
        if self
            .armed
            .is_some_and(|(w, i, t)| w == what && i == index && t.elapsed() < CONFIRM_TIME)
        {
            self.armed = None;
            return true;
        }
        self.armed = Some((what, index, Instant::now()));
        self.show(msg.to_string());
        false
    }

    // ----- projects, files, tabs -----

    fn open_folder(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let fs = self.renderer.font_system();
        let ex = Explorer::new(path.clone(), fs, self.ui_font, self.family.clone());
        self.explorer = Some(ex);
        self.explorer_visible = true;
        config::remember_project(&path);
        self.index.rebuild(&path);
        self.relayout();
    }

    fn project_root(&self) -> Option<&Path> {
        self.explorer.as_ref().map(|e| e.root())
    }

    /// Opens a file in a tab (or switches to it if it is already open).
    fn open_path(&mut self, path: &Path) {
        if let Some(i) = self.tabs.iter().position(|t| t.doc.path() == Some(path)) {
            self.activate(i);
        } else {
            let doc = if path.exists() {
                match Document::open(path) {
                    Ok(d) => d,
                    Err(e) => {
                        self.show(format!("{} kann nicht geöffnet werden: {e}", file_name(path)));
                        return;
                    }
                }
            } else {
                let mut d = Document::default();
                if let Err(e) = d.save_as(path.to_path_buf()) {
                    self.show(format!("{} kann nicht angelegt werden: {e}", file_name(path)));
                    return;
                }
                d
            };
            let fs = self.renderer.font_system();
            let mut view = EditorView::new(doc, fs, self.font, self.family.clone());
            view.set_area(self.layout.editor);
            if self.tabs[self.active].is_untouched() {
                self.tabs[self.active] = view;
                self.activate(self.active);
            } else {
                self.tabs.push(view);
                self.mru.push(self.tabs.len() - 1);
                self.activate(self.tabs.len() - 1);
            }
        }
        if let Some(ex) = &mut self.explorer {
            ex.reveal(path);
        }
        self.focus = Focus::Editor;
        self.completion = None;
        self.lsp_open_all();
    }

    fn activate(&mut self, index: usize) {
        self.active = index;
        if self.cycling.is_none() {
            self.mru.retain(|&i| i != index);
            self.mru.insert(0, index);
        }
        if let Some(ex) = &mut self.explorer {
            ex.invalidate();
        }
        self.window.request_redraw();
    }

    fn new_file(&mut self) {
        let fs = self.renderer.font_system();
        let mut view = EditorView::new(Document::default(), fs, self.font, self.family.clone());
        view.set_area(self.layout.editor);
        self.tabs.push(view);
        self.mru.push(self.tabs.len() - 1);
        self.activate(self.tabs.len() - 1);
        self.focus = Focus::Editor;
    }

    fn close_tab(&mut self, index: usize) {
        let tab = &self.tabs[index];
        if tab.doc.is_dirty() {
            let msg = format!("{} ungespeichert! Nochmal Strg+W verwirft", tab.file_name());
            if !self.confirm("close-tab", index, &msg) {
                return;
            }
        }
        let tab = self.tabs.remove(index);
        if let Some(p) = tab.doc.path() {
            self.closed.push(p.to_path_buf());
            if let Some(server) = &mut self.lsp {
                server.close(p);
            }
        }
        self.completion = None;
        self.mru.retain(|&i| i != index);
        for i in &mut self.mru {
            if *i > index {
                *i -= 1;
            }
        }
        if self.tabs.is_empty() {
            let fs = self.renderer.font_system();
            let mut view = EditorView::new(Document::default(), fs, self.font, self.family.clone());
            view.set_area(self.layout.editor);
            self.tabs.push(view);
            self.mru = vec![0];
        }
        let next = self.mru.first().copied().unwrap_or(0);
        self.activate(next);
    }

    fn close_all(&mut self) {
        let mut i = self.tabs.len();
        let mut kept = 0;
        while i > 0 {
            i -= 1;
            if self.tabs[i].doc.is_dirty() {
                kept += 1;
            } else if self.tabs.len() > 1 || !self.tabs[i].is_untouched() {
                self.armed = None;
                self.close_tab(i);
            }
        }
        if kept > 0 {
            self.show(format!("{kept} Datei(en) mit ungespeicherten Änderungen bleiben offen."));
        }
    }

    fn cycle_tabs(&mut self, back: bool) {
        let n = self.mru.len();
        if n < 2 {
            return;
        }
        let pos = match self.cycling {
            None if back => n - 1,
            None => 1,
            Some(p) if back => (p + n - 1) % n,
            Some(p) => (p + 1) % n,
        };
        self.cycling = Some(pos);
        self.active = self.mru[pos];
        if let Some(ex) = &mut self.explorer {
            ex.invalidate();
        }
    }

    fn finish_cycling(&mut self) {
        if self.cycling.take().is_some() {
            let a = self.active;
            self.activate(a);
        }
    }

    fn save(&mut self) {
        if self.editor().doc.path().is_none() {
            self.save_as();
            return;
        }
        let ed = &mut self.tabs[self.active];
        match ed.doc.save() {
            Ok(()) => {
                let name = ed.file_name();
                if let (Some(server), Some(path)) = (&mut self.lsp, ed.doc.path()) {
                    server.saved(path);
                }
                self.show(format!("{name} gespeichert"));
            }
            Err(e) => self.show(format!("Speichern fehlgeschlagen: {e}")),
        }
    }

    fn dialog_dir(&self) -> Option<PathBuf> {
        self.editor()
            .doc
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.project_root().map(Path::to_path_buf))
    }

    fn save_as(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Speichern unter")
            .set_file_name(self.editor().file_name());
        if let Some(dir) = self.dialog_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else { return };
        match self.editor_mut().save_as(path.clone()) {
            Ok(()) => {
                self.show(format!("{} gespeichert", file_name(&path)));
                self.lsp_open_all();
                if let Some(ex) = &mut self.explorer {
                    ex.refresh();
                    ex.reveal(&path);
                }
            }
            Err(e) => self.show(format!("Speichern fehlgeschlagen: {e}")),
        }
    }

    fn pick_file(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Datei öffnen");
        if let Some(dir) = self.dialog_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_path(&path);
        }
    }

    fn pick_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Ordner öffnen");
        if let Some(dir) = self.project_root() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_folder() {
            self.open_folder(&path);
        }
    }

    fn confirm_close_window(&mut self) -> bool {
        let dirty = self.tabs.iter().filter(|t| t.doc.is_dirty()).count();
        if dirty == 0 {
            return true;
        }
        let msg = format!("{dirty} Datei(en) ungespeichert! Nochmal schließen verwirft");
        let ok = self.confirm("close-window", 0, &msg);
        self.window.request_redraw();
        ok
    }

    // ----- keyboard -----

    fn key_event(&mut self, event: &KeyEvent) {
        let key = event.key_without_modifiers();
        if event.state == ElementState::Released {
            match key {
                // Alt pressed and released alone: leader menu.
                Key::Named(NamedKey::Alt) if self.alt_alone => {
                    self.alt_alone = false;
                    self.overlay = match self.overlay {
                        Overlay::Leader { .. } => Overlay::None,
                        _ => Overlay::Leader { group: "", since: Instant::now(), shown: false },
                    };
                    self.window.request_redraw();
                }
                Key::Named(NamedKey::Control) => {
                    self.finish_cycling();
                    self.window.request_redraw();
                }
                _ => {}
            }
            return;
        }
        if key == Key::Named(NamedKey::Alt) {
            if !event.repeat {
                self.alt_alone = true;
            }
            return;
        }
        self.alt_alone = false;
        self.latency.input();
        self.window.request_redraw();

        match &self.overlay {
            Overlay::Leader { group, .. } => {
                let group = *group;
                self.leader_key(group, &key);
                return;
            }
            Overlay::Recent(list) => {
                let list = list.clone();
                self.overlay = Overlay::None;
                if let Key::Character(c) = &key
                    && let Some(n) = c.chars().next().and_then(|c| c.to_digit(10))
                        && let Some(p) = list.get((n as usize).wrapping_sub(1)) {
                            self.open_folder(p);
                        }
                return;
            }
            Overlay::Palette => {
                self.palette_key(event, &key);
                return;
            }
            Overlay::None => {}
        }

        let mods = self.modifiers;
        self.tooltip = None;
        if self.focus == Focus::Editor && !mods.control_key() && !mods.alt_key()
            && self.completion_key(&key) {
                return;
            }
        if self.focus == Focus::Explorer && !mods.control_key() && !mods.alt_key()
            && self.explorer_key(&key) {
                return;
            }

        let cmd = self
            .keymap
            .handle(event, mods)
            .or_else(|| Keymap::text(event, mods).map(Command::Insert));
        if self.keymap.chord_pending() {
            self.show("Strg+K … (C = auskommentieren, U = Kommentar entfernen, D = formatieren, I = Info)");
            return;
        }
        // The chord hint has done its job once the second key arrived.
        if self.message.as_ref().is_some_and(|(m, _)| m.starts_with("Strg+K …")) {
            self.message = None;
        }
        if let Some(cmd) = cmd {
            if !matches!(cmd, Command::CycleTabs { .. }) {
                self.finish_cycling();
            }
            self.run(cmd);
        }
    }

    /// Keys while the explorer has focus. Returns false if the key was not used.
    fn explorer_key(&mut self, key: &Key) -> bool {
        let Some(ex) = &mut self.explorer else { return false };
        let page = 10;
        let open = match key {
            Key::Named(NamedKey::ArrowUp) => {
                ex.move_selection(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                ex.move_selection(1);
                None
            }
            Key::Named(NamedKey::PageUp) => {
                ex.move_selection(-page);
                None
            }
            Key::Named(NamedKey::PageDown) => {
                ex.move_selection(page);
                None
            }
            Key::Named(NamedKey::Home) => {
                ex.move_selection(isize::MIN / 2);
                None
            }
            Key::Named(NamedKey::End) => {
                ex.move_selection(isize::MAX / 2);
                None
            }
            Key::Named(NamedKey::ArrowRight) => {
                ex.expand_selected(true);
                None
            }
            Key::Named(NamedKey::ArrowLeft) => {
                ex.expand_selected(false);
                None
            }
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => ex.activate_selected(),
            Key::Named(NamedKey::Escape) => {
                self.focus = Focus::Editor;
                return true;
            }
            Key::Character(_) => return true, // type-to-search comes later; never edit the file
            _ => return false,
        };
        if let Some(path) = open {
            self.open_path(&path);
        }
        true
    }

    // ----- language server -----

    /// Starts the C# server when the first C# file is open, and tells it about open files.
    fn lsp_open_all(&mut self) {
        let wants = self.tabs.iter().any(|t| t.is_csharp() && t.doc.path().is_some());
        if !wants {
            return;
        }
        if self.lsp.is_none() && !self.lsp_failed {
            let root = self
                .project_root()
                .map(Path::to_path_buf)
                .or_else(|| self.editor().doc.path().and_then(Path::parent).map(Path::to_path_buf));
            if let Some(root) = root {
                let proxy = self.proxy.clone();
                match lsp::LanguageServer::start_csharp(&root, move || {
                    let _ = proxy.send_event(UserEvent::Lsp);
                }) {
                    Ok(server) => self.lsp = Some(server),
                    Err(e) => {
                        self.lsp_failed = true;
                        self.show(e);
                    }
                }
            }
        }
        let Some(server) = &mut self.lsp else { return };
        for t in &self.tabs {
            if let (true, Some(path)) = (t.is_csharp(), t.doc.path())
                && !server.is_open(path) {
                    server.open(path, t.doc.text().to_string(), t.doc.version());
                }
        }
    }

    /// Sends the active document right away (before asking the server something about it).
    fn lsp_sync_active(&mut self) -> bool {
        let ed = &self.tabs[self.active];
        let (Some(server), Some(path)) = (&mut self.lsp, ed.doc.path()) else { return false };
        if !server.is_open(path) {
            return false;
        }
        server.sync(path, ed.doc.text(), ed.doc.version());
        true
    }

    fn lsp_after_edit(&mut self) {
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (&mut self.lsp, ed.doc.path()) {
            server.changed(path, ed.doc.version());
        }
    }

    fn lsp_ready(&self) -> bool {
        self.lsp.as_ref().is_some_and(|s| s.status == lsp::Status::Ready)
    }

    /// Opens, filters or closes the completion list after a command.
    fn update_completion(&mut self, typed: Option<&str>, trigger: Option<Option<char>>) {
        let ed = &self.tabs[self.active];
        if !ed.is_csharp() || ed.doc.path().is_none() || ed.doc.selection().len() > 1 {
            self.completion = None;
            return;
        }
        let rope = ed.doc.text();
        let head = ed.doc.selection().primary().head;
        if let Some(c) = &mut self.completion {
            let same_doc = ed.doc.path() == Some(c.path.as_path());
            let same_line = same_doc
                && c.anchor <= head
                && rope.char_to_line(c.anchor) == rope.char_to_line(head);
            let prefix: String = if same_line { rope.slice(c.anchor..head).to_string() } else { String::new() };
            let word = prefix.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
            if same_line && word {
                c.filter(&prefix);
                if c.is_empty() {
                    self.completion = None;
                }
                return;
            }
            self.completion = None;
        }
        let trigger = match (trigger, typed) {
            (Some(t), _) => Some(t),
            (None, Some(".")) => Some(Some('.')),
            (None, Some(t)) => {
                let mut chars = t.chars();
                let ident = matches!((chars.next(), chars.next()), (Some(ch), None) if ch.is_alphabetic() || ch == '_');
                (ident && ed.word_start() + 1 == head).then_some(None)
            }
            _ => None,
        };
        let Some(trigger) = trigger else { return };
        if !self.lsp_ready() {
            if typed.is_none() {
                self.show("Der C#-Server lädt noch das Projekt …");
            }
            return;
        }
        let anchor = self.editor().word_start();
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (&mut self.lsp, ed.doc.path()) {
            server.completion(path, ed.doc.text(), head, anchor, trigger);
        }
    }

    fn accept_completion(&mut self) {
        let Some(c) = self.completion.take() else { return };
        let Some(insert) = c.selected_insert() else { return };
        let ed = self.editor_mut();
        let head = ed.doc.selection().primary().head;
        ed.doc.replace_range(c.anchor, head, &insert);
        ed.after_edit();
        ed.ensure_cursor_visible();
        self.lsp_after_edit();
    }

    /// Keys while the completion list is open. Returns true if the key was used.
    fn completion_key(&mut self, key: &Key) -> bool {
        let Some(c) = &mut self.completion else { return false };
        match key {
            Key::Named(NamedKey::ArrowUp) => c.move_selection(-1),
            Key::Named(NamedKey::ArrowDown) => c.move_selection(1),
            Key::Named(NamedKey::PageUp) => c.move_selection(-9),
            Key::Named(NamedKey::PageDown) => c.move_selection(9),
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) => self.accept_completion(),
            Key::Named(NamedKey::Escape) => self.completion = None,
            _ => return false,
        }
        true
    }

    fn request_definition(&mut self, pos: usize) {
        if !self.tabs[self.active].is_csharp() {
            self.show("Gehe zu Definition gibt es bisher nur für C#.");
            return;
        }
        if !self.lsp_ready() {
            self.show("Der C#-Server lädt noch das Projekt …");
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (&mut self.lsp, ed.doc.path()) {
            server.definition(path, ed.doc.text(), pos);
        }
    }

    fn request_hover(&mut self, pos: usize, x: f32, y: f32) {
        if !self.tabs[self.active].is_csharp() || !self.lsp_ready() {
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (&mut self.lsp, ed.doc.path()) {
            server.hover(path, ed.doc.text(), pos, x, y);
        }
    }

    fn request_format(&mut self) {
        if !self.tabs[self.active].is_csharp() {
            self.show("Formatieren gibt es bisher nur für C#.");
            return;
        }
        if !self.lsp_ready() {
            self.show("Der C#-Server lädt noch das Projekt …");
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        let spaces = ed.doc.indent_unit != "\t";
        if let (Some(server), Some(path)) = (&mut self.lsp, ed.doc.path()) {
            server.format(path, ed.doc.version(), ed.doc.tab_width, spaces);
        }
    }

    /// Handles everything the language server sent.
    fn lsp_poll(&mut self) {
        let Some(server) = &mut self.lsp else { return };
        let events = server.poll();
        for event in events {
            match event {
                lsp::Event::Status => self.lsp_open_all(),
                lsp::Event::Message(m) => self.show(m),
                lsp::Event::Diagnostics(path) => self.apply_diagnostics(&path),
                lsp::Event::Completion { path, anchor, items } => {
                    let ed = &self.tabs[self.active];
                    let head = ed.doc.selection().primary().head;
                    let rope = ed.doc.text();
                    let valid = ed.doc.path() == Some(path.as_path())
                        && anchor <= head
                        && rope.char_to_line(anchor) == rope.char_to_line(head);
                    if valid && !items.is_empty() {
                        let prefix = rope.slice(anchor..head).to_string();
                        let fs = self.renderer.font_system();
                        let mut c = completion::Completion::new(fs, self.ui_font, path, anchor, items);
                        c.filter(&prefix);
                        self.completion = (!c.is_empty()).then_some(c);
                    }
                }
                lsp::Event::Definition { path, start } => {
                    self.open_path(&path);
                    let ed = self.editor_mut();
                    if ed.doc.path() == Some(path.as_path()) {
                        let pos = wyvern_lsp::from_lsp(ed.doc.text(), &start);
                        ed.jump_to(pos);
                    }
                }
                lsp::Event::DefinitionNotFound => self.show("Keine Definition gefunden"),
                lsp::Event::Hover { text, x, y } => {
                    let fs = self.renderer.font_system();
                    self.tooltip = Some(completion::Tooltip::new(fs, self.ui_font, &self.family, &self.theme, &text, x, y));
                }
                lsp::Event::Format { path, version, edits } => {
                    let Some(i) = self.tabs.iter().position(|t| t.doc.path() == Some(path.as_path())) else { continue };
                    let ed = &mut self.tabs[i];
                    if ed.doc.version() != version {
                        continue; // text changed meanwhile; formatting would be wrong
                    }
                    let rope = ed.doc.text();
                    let changes = edits
                        .iter()
                        .map(|(s, e, text)| wyvern_buffer::Change {
                            start: wyvern_lsp::from_lsp(rope, s),
                            end: wyvern_lsp::from_lsp(rope, e),
                            text: text.replace("\r\n", "\n"),
                        })
                        .collect::<Vec<_>>();
                    let n = changes.len();
                    ed.doc.apply_changes(changes);
                    ed.after_edit();
                    if let Some(server) = &mut self.lsp {
                        server.changed(&path, self.tabs[i].doc.version());
                    }
                    self.show(if n == 0 { "Schon formatiert".to_string() } else { "Formatiert".to_string() });
                }
            }
        }
        self.window.request_redraw();
    }

    /// Converts the server's problem list for a file into ranges in its tab.
    fn apply_diagnostics(&mut self, path: &Path) {
        let Some(server) = &self.lsp else { return };
        let list = server.diagnostics.get(path).cloned().unwrap_or_default();
        if let Some(t) = self.tabs.iter_mut().find(|t| t.doc.path() == Some(path)) {
            let rope = t.doc.text();
            t.diagnostics = list
                .iter()
                .filter(|d| d.severity <= 3)
                .map(|d| editor::DiagSpan {
                    start: wyvern_lsp::from_lsp(rope, &d.start),
                    end: wyvern_lsp::from_lsp(rope, &d.end),
                    severity: d.severity,
                    message: if d.code.is_empty() { d.message.clone() } else { format!("{}: {}", d.code, d.message) },
                })
                .collect();
        }
    }

    /// Periodic work for the language server: send pending changes, ask for diagnostics,
    /// show hover info when the mouse rests. Returns when to wake up next.
    fn lsp_tick(&mut self, now: Instant) -> Option<Instant> {
        let mut wake = None;
        if let Some(server) = &mut self.lsp {
            for path in server.due_syncs(now) {
                if let Some(t) = self.tabs.iter().find(|t| t.doc.path() == Some(path.as_path())) {
                    server.sync(&path, t.doc.text(), t.doc.version());
                }
            }
            wake = server.tick(now);
        }
        if let Some((since, x, y)) = self.hover_rest {
            let due = since + HOVER_DELAY;
            if now >= due {
                self.hover_rest = None;
                if self.tooltip.is_none() && self.editor().contains(x, y) && matches!(self.overlay, Overlay::None) {
                    let pos = self.editor().hit_test(x, y);
                    let rope = self.editor().doc.text();
                    let on_word = pos < rope.len_chars() && {
                        let c = rope.char(pos);
                        c.is_alphanumeric() || c == '_'
                    };
                    if on_word {
                        self.request_hover(pos, x, y);
                    }
                }
            } else {
                wake = Some(wake.map_or(due, |w: Instant| w.min(due)));
            }
        }
        wake
    }

    // ----- search field -----

    fn open_palette(&mut self, prefix: &'static str) {
        self.palette.reset(prefix);
        self.overlay = Overlay::Palette;
        self.refresh_palette();
    }

    fn refresh_palette(&mut self) {
        let open: Vec<PathBuf> = self
            .mru
            .iter()
            .filter_map(|&i| self.tabs.get(i)?.doc.path().map(Path::to_path_buf))
            .collect();
        let ed = &self.tabs[self.active];
        let symbols = if self.palette.query.starts_with('@') { ed.symbols() } else { Vec::new() };
        let sources = palette::Sources {
            files: self.index.files(),
            root: self.explorer.as_ref().map(|e| e.root()),
            open,
            has_symbols: ed.language_name() != "Text",
            symbols,
            line_count: ed.doc.text().len_lines(),
        };
        self.palette.update(&sources);
    }

    fn palette_key(&mut self, event: &KeyEvent, key: &Key) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        match key {
            Key::Named(NamedKey::Escape) => self.overlay = Overlay::None,
            Key::Character(c) if ctrl && c.as_str() == "q" => self.overlay = Overlay::None,
            Key::Named(NamedKey::Enter) => {
                let action = self.palette.selected_action();
                self.accept_palette(action);
            }
            Key::Named(NamedKey::ArrowUp) => self.palette.move_selection(-1),
            Key::Named(NamedKey::ArrowDown) => self.palette.move_selection(1),
            Key::Named(NamedKey::PageUp) => self.palette.move_selection(-10),
            Key::Named(NamedKey::PageDown) => self.palette.move_selection(10),
            Key::Named(NamedKey::Backspace) => {
                if ctrl {
                    self.palette.query.clear();
                } else {
                    self.palette.query.pop();
                }
                self.refresh_palette();
            }
            Key::Character(c) if ctrl && c.as_str() == "v" => {
                if let Some((text, _)) = self.get_clipboard() {
                    let line = text.lines().next().unwrap_or("").to_string();
                    self.palette.query.push_str(&line);
                    self.refresh_palette();
                }
            }
            _ => {
                if let Some(text) = Keymap::text(event, mods) {
                    self.palette.query.push_str(&text);
                    self.refresh_palette();
                }
            }
        }
    }

    fn accept_palette(&mut self, action: palette::Action) {
        use palette::{Action as A, Cmd};
        self.overlay = Overlay::None;
        match action {
            A::OpenFile(rel) => {
                if let Some(root) = self.project_root() {
                    let path = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                    self.open_path(&path);
                }
            }
            A::Run(Cmd::Key(cmd)) => self.run(cmd),
            A::Run(Cmd::Leader(action)) => self.leader_action(action),
            A::Line(line, col) => {
                self.editor_mut().jump_to_line(line, col);
                self.focus = Focus::Editor;
            }
            A::Byte(byte) => {
                self.editor_mut().jump_to_byte(byte);
                self.focus = Focus::Editor;
            }
            A::Nothing => {}
        }
    }


    fn leader_key(&mut self, group: &'static str, key: &Key) {
        self.overlay = Overlay::None;
        let c = match key {
            Key::Named(NamedKey::Escape) => return,
            Key::Character(s) => s.chars().next(),
            _ => None,
        };
        let Some(c) = c else { return };
        match leader::find(group, c) {
            Some(leader::Action::Group(g)) => {
                // Same rule as the first level: the popup only appears if the user pauses.
                self.overlay = Overlay::Leader { group: g, since: Instant::now(), shown: false };
            }
            Some(action) => self.leader_action(action),
            None => self.show(format!("Alt → {group}{c} ist nicht belegt (Alt → h zeigt alle Kürzel)")),
        }
    }

    fn leader_action(&mut self, action: leader::Action) {
        use leader::Action::*;
        match action {
            Group(_) => {}
            ToggleExplorer => {
                if self.explorer.is_none() {
                    self.show("Kein Ordner offen: Alt → p → o öffnet einen.");
                    return;
                }
                self.explorer_visible = !self.explorer_visible;
                self.focus = if self.explorer_visible { Focus::Explorer } else { Focus::Editor };
                self.relayout();
            }
            FocusExplorer => {
                if self.explorer.is_some() {
                    self.explorer_visible = true;
                    self.focus = Focus::Explorer;
                    self.relayout();
                }
            }
            OpenFolder => self.pick_folder(),
            RecentProjects => {
                let list = config::recent_projects();
                if list.is_empty() {
                    self.show("Noch keine zuletzt geöffneten Projekte.");
                } else {
                    self.overlay = Overlay::Recent(list);
                }
            }
            Palette(prefix) => self.open_palette(prefix),
            OpenFile => self.pick_file(),
            NewFile => self.new_file(),
            SaveAs => self.save_as(),
            CloseAll => self.close_all(),
            Later(msg) => self.show(msg),
        }
    }

    fn run(&mut self, cmd: Command) {
        let mut follow_cursor = true;
        let typed = if let Command::Insert(t) = &cmd { Some(t.clone()) } else { None };
        let keeps_completion =
            matches!(cmd, Command::Insert(_) | Command::Backspace | Command::TriggerCompletion);
        let mut trigger = None;
        let ed = &mut self.tabs[self.active];
        match cmd {
            Command::Move(m, extend) => {
                let m = match m {
                    Movement::PageUp(_) => Movement::PageUp(ed.page_lines()),
                    Movement::PageDown(_) => Movement::PageDown(ed.page_lines()),
                    m => m,
                };
                ed.doc.move_cursors(m, extend);
            }
            Command::Insert(text) => ed.doc.insert(&text),
            Command::Newline => ed.doc.newline(),
            Command::NewlineBelow => {
                ed.doc.move_cursors(Movement::LineEnd, false);
                ed.doc.newline();
            }
            Command::NewlineAbove => {
                let line = ed.doc.text().char_to_line(ed.doc.selection().primary().head);
                if line == 0 {
                    ed.doc.set_cursor(0);
                    ed.doc.insert("\n");
                    ed.doc.set_cursor(0);
                } else {
                    ed.doc.move_cursors(Movement::Up, false);
                    ed.doc.move_cursors(Movement::LineEnd, false);
                    ed.doc.newline();
                }
            }
            Command::Indent => ed.doc.indent(),
            Command::Outdent => ed.doc.outdent(),
            Command::Backspace => ed.doc.backspace(),
            Command::Delete => ed.doc.delete_forward(),
            Command::DeleteWord { forward } => ed.doc.delete_word(forward),
            Command::Undo => {
                ed.doc.undo();
            }
            Command::Redo => {
                ed.doc.redo();
            }
            Command::Copy => {
                let copied = ed.doc.copy();
                self.set_clipboard(copied);
            }
            Command::Cut => {
                let copied = ed.doc.cut();
                self.set_clipboard(copied);
            }
            Command::Paste => {
                if let Some((text, line_mode)) = self.get_clipboard() {
                    self.editor_mut().doc.paste(&text, line_mode);
                }
            }
            Command::SelectAll => {
                ed.doc.select_all();
                follow_cursor = false;
            }
            Command::AddNextOccurrence => ed.doc.add_next_occurrence(),
            Command::Collapse => {
                ed.doc.collapse_to_primary();
                self.message = None;
            }
            Command::MoveLines(dir) => ed.doc.move_lines(dir),
            Command::DuplicateLines(dir) => ed.doc.duplicate_lines(dir),
            Command::DeleteLines => ed.doc.delete_lines(),
            Command::Comment(mode) => {
                let token = ed.line_comment();
                ed.doc.comment_lines(token, mode);
            }
            Command::Save => {
                follow_cursor = false;
                self.save();
            }
            Command::SaveAs => {
                follow_cursor = false;
                self.save_as();
            }
            Command::OpenFile => {
                follow_cursor = false;
                self.pick_file();
            }
            Command::NewFile => self.new_file(),
            Command::CloseTab => {
                follow_cursor = false;
                let a = self.active;
                self.close_tab(a);
            }
            Command::CycleTabs { back } => {
                follow_cursor = false;
                self.cycle_tabs(back);
            }
            Command::OpenPalette(prefix) => {
                follow_cursor = false;
                self.open_palette(prefix);
            }
            Command::ReopenClosed => {
                if let Some(p) = self.closed.pop() {
                    self.open_path(&p);
                }
            }
            Command::Scroll(n) => {
                ed.scroll_by(n);
                follow_cursor = false;
            }
            Command::FontSize(d) => {
                self.font_pt = (self.font_pt + d as f32).clamp(8.0, 40.0);
                self.apply_font();
                follow_cursor = false;
            }
            Command::FontSizeReset => {
                self.font_pt = DEFAULT_FONT_SIZE;
                self.apply_font();
                follow_cursor = false;
            }
            Command::NotYet(msg) => {
                self.show(msg);
                follow_cursor = false;
            }
            Command::TriggerCompletion => trigger = Some(None),
            Command::GotoDefinition => {
                follow_cursor = false;
                let pos = self.editor().doc.selection().primary().head;
                self.request_definition(pos);
            }
            Command::Format => {
                follow_cursor = false;
                self.request_format();
            }
            Command::QuickInfo => {
                follow_cursor = false;
                let ed = self.editor();
                let pos = ed.doc.selection().primary().head;
                let (x, y) = ed.screen_pos(pos);
                let y = y + ed.line_height();
                self.request_hover(pos, x, y);
            }
        }
        let ed = self.editor_mut();
        ed.after_edit();
        if follow_cursor {
            ed.ensure_cursor_visible();
        }
        self.lsp_after_edit();
        if !keeps_completion {
            self.completion = None;
        }
        self.update_completion(typed.as_deref(), trigger);
    }

    fn set_clipboard(&mut self, (text, line_mode): (String, bool)) {
        if text.is_empty() {
            return;
        }
        if let Some(cb) = &mut self.clipboard
            && let Err(e) = cb.set_text(text.clone()) {
                log::warn!("clipboard: {e}");
            }
        self.last_copy = Some((text, line_mode));
    }

    fn get_clipboard(&mut self) -> Option<(String, bool)> {
        let text = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok());
        match (text, &self.last_copy) {
            // Our own copy: keep the whole-line flag.
            (Some(t), Some((own, line_mode))) if t.replace("\r\n", "\n") == *own => {
                Some((own.clone(), *line_mode))
            }
            (Some(t), _) => Some((t, false)),
            (None, Some(own)) => Some(own.clone()),
            (None, None) => None,
        }
    }

    // ----- mouse -----

    fn mouse_button(&mut self, button: MouseButton, pressed: bool) {
        if !pressed {
            self.mouse.dragging = false;
            return;
        }
        self.alt_alone = false;
        self.tooltip = None;
        self.completion = None;
        if matches!(self.overlay, Overlay::Palette) && button == MouseButton::Left
            && let Some(action) = self.palette.action_at(self.mouse.x, self.mouse.y) {
                self.accept_palette(action);
                self.window.request_redraw();
                return;
            }
        if !matches!(self.overlay, Overlay::None) {
            self.overlay = Overlay::None;
            self.window.request_redraw();
            return;
        }
        let (x, y) = (self.mouse.x, self.mouse.y);
        self.latency.input();
        self.window.request_redraw();

        let t = self.layout.tabs;
        if y >= t.y && y < t.y + t.h && x >= t.x {
            if let Some(i) = self.tab_spans.iter().position(|&(a, b)| x >= a && x < b) {
                match button {
                    MouseButton::Middle => self.close_tab(i),
                    MouseButton::Left => {
                        self.activate(i);
                        self.focus = Focus::Editor;
                    }
                    _ => {}
                }
            }
            return;
        }

        if let Some(ex) = &mut self.explorer
            && self.explorer_visible && ex.contains(x, y) {
                self.focus = Focus::Explorer;
                if button == MouseButton::Left
                    && let Some(i) = ex.row_at(y)
                        && let Some(path) = ex.activate(i) {
                            self.open_path(&path);
                        }
                return;
            }

        if button != MouseButton::Left || !self.editor().contains(x, y) {
            return;
        }
        self.focus = Focus::Editor;
        let pos = self.editor().hit_test(x, y);
        let now = Instant::now();
        let double = self
            .mouse
            .last_click
            .is_some_and(|(t, p)| now.duration_since(t) < DOUBLE_CLICK && p == pos);
        self.mouse.last_click = Some((now, pos));
        let (alt, shift) = (self.modifiers.alt_key(), self.modifiers.shift_key());
        if self.modifiers.control_key() {
            self.tabs[self.active].doc.set_cursor(pos);
            self.request_definition(pos);
            return;
        }
        let doc = &mut self.tabs[self.active].doc;
        if alt {
            doc.toggle_cursor(pos);
        } else if shift {
            doc.extend_primary_to(pos);
            self.mouse.dragging = true;
        } else if double {
            doc.select_word_at(pos);
        } else {
            doc.set_cursor(pos);
            self.mouse.dragging = true;
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x * 3.0, y * 3.0),
            MouseScrollDelta::PixelDelta(p) => {
                let lh = self.font.line_h as f64;
                ((p.x / lh) as f32, (p.y / lh) as f32)
            }
        };
        let (x, y) = (self.mouse.x, self.mouse.y);
        if let Some(ex) = &mut self.explorer
            && self.explorer_visible && ex.contains(x, y) {
                ex.scroll_by(-dy.round() as isize);
                self.window.request_redraw();
                return;
            }
        let shift = self.modifiers.shift_key();
        let cw = self.font.cell_w;
        let ed = self.editor_mut();
        if shift {
            ed.scroll_x_by(-dy * cw * 2.0);
        } else {
            ed.scroll_by(-dy.round() as isize);
            ed.scroll_x_by(-dx * cw);
        }
        self.window.request_redraw();
    }

    // ----- drawing -----

    fn update_title(&mut self) {
        let ed = self.editor();
        let dirty = if ed.doc.is_dirty() { "● " } else { "" };
        let project = self.project_root().map(|r| format!(" – {}", file_name(r))).unwrap_or_default();
        let title = format!("{dirty}{}{project} – Wyvern", ed.file_name());
        if title != self.title {
            self.window.set_title(&title);
            self.title = title;
        }
    }

    fn update_tab_bar(&mut self) {
        let cw = self.ui_font.cell_w;
        let mut x = self.layout.tabs.x;
        let mut spans: Vec<(String, Rgba)> = Vec::new();
        self.tab_spans.clear();
        for (i, t) in self.tabs.iter().enumerate() {
            let dirty = if t.doc.is_dirty() { " ●" } else { "  " };
            let label = format!("  {}{dirty} ", t.file_name());
            let w = label.chars().count() as f32 * cw;
            self.tab_spans.push((x, x + w));
            x += w;
            let color =
                if i == self.active { self.theme.tab_text_active } else { self.theme.tab_text };
            spans.push((label, color));
        }
        let attrs = Attrs::new().family(Family::Name(&self.family));
        self.tab_bar.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.tab_bar.shape_until_scroll(self.renderer.font_system(), false);
    }

    fn update_status(&mut self) {
        let ed = self.editor();
        let rope = ed.doc.text();
        let head = ed.doc.selection().primary().head;
        let line = rope.char_to_line(head);
        let col = head - rope.line_to_char(line);
        let cursors = ed.doc.selection().len();
        let mut left = format!("Zeile {}, Spalte {}", line + 1, col + 1);
        if cursors > 1 {
            left.push_str(&format!("   {cursors} Cursor"));
        }
        if self.focus == Focus::Explorer {
            left.push_str("   ·   Explorer (Esc = zurück zum Editor)");
        }
        if let Some((msg, _)) = &self.message {
            left.push_str("   ·   ");
            left.push_str(msg);
        } else if let Some(d) = ed.diagnostic_at_cursor() {
            left.push_str("   ·   ");
            left.push_str(&d.message);
        }
        let errors = ed.diagnostics.iter().filter(|d| d.severity == 1).count();
        let warnings = ed.diagnostics.iter().filter(|d| d.severity == 2).count();
        let problems = if errors + warnings > 0 { format!("✖ {errors}  ⚠ {warnings}  ·  ") } else { String::new() };
        let language = match (&self.lsp, ed.is_csharp()) {
            (Some(server), true) => server.status_text(),
            _ => ed.language_name().to_string(),
        };
        let ending = match ed.doc.line_ending() {
            wyvern_buffer::LineEnding::Lf => "LF",
            wyvern_buffer::LineEnding::CrLf => "CRLF",
        };
        let ram = metrics::ram_mb().map_or("–".into(), |mb| format!("{mb:.0} MB"));
        let right = format!(
            "{problems}{language}  ·  {ending}  ·  RAM {ram}  ·  Taste→Bild {} (Ø {})",
            metrics::ms(self.latency.last()),
            metrics::ms(self.latency.average()),
        );
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let fs = self.renderer.font_system();
        self.status_left.set_text(&left, &attrs, Shaping::Advanced, None);
        self.status_left.shape_until_scroll(fs, false);
        self.status_right.set_text(&right, &attrs, Shaping::Advanced, None);
        self.status_right.shape_until_scroll(fs, false);
    }

    fn update_popup(&mut self) -> bool {
        let (title, entries): (String, Vec<(String, String)>) = match &self.overlay {
            Overlay::Leader { group, shown: true, .. } => (
                leader::title(group),
                leader::items(group)
                    .iter()
                    .map(|i| (i.key.to_string(), i.label.to_string()))
                    .collect(),
            ),
            Overlay::Recent(list) => (
                "Zuletzt geöffnet (Ziffer wählen, Esc = abbrechen)".into(),
                list.iter()
                    .enumerate()
                    .map(|(i, p)| ((i + 1).to_string(), p.display().to_string()))
                    .collect(),
            ),
            _ => return false,
        };
        let fs = self.renderer.font_system();
        self.popup.set(fs, &self.theme, &self.family, &title, &entries);
        true
    }

    fn redraw(&mut self) {
        self.update_title();
        self.update_tab_bar();
        self.update_status();
        let popup_visible = self.update_popup();
        if matches!(self.overlay, Overlay::Palette) {
            let (w, _) = self.renderer.size();
            let cells = Palette::width_cells(w as f32, self.ui_font);
            let fs = self.renderer.font_system();
            self.palette.prepare(fs, &self.theme, &self.family, cells);
        }
        if let Some(c) = &mut self.completion {
            let fs = self.renderer.font_system();
            c.prepare(fs, &self.theme, &self.family);
        }
        let active_path = self.editor().doc.path().map(Path::to_path_buf);
        {
            let theme = &self.theme;
            let fs = self.renderer.font_system();
            self.tabs[self.active].prepare(fs, theme);
            if let Some(ex) = &mut self.explorer {
                ex.prepare(self.renderer.font_system(), theme, active_path.as_deref());
            }
        }

        let theme = &self.theme;
        let scale = self.renderer.scale_factor();
        let l = self.layout;
        let mut under = Vec::with_capacity(64);
        let mut over = Vec::with_capacity(16);
        let mut overlay = Vec::new();
        let mut text: Vec<TextBlock<'_>> = Vec::with_capacity(8);
        let mut overlay_text: Vec<TextBlock<'_>> = Vec::new();

        if let (Some(ex), true) = (&self.explorer, self.explorer_visible) {
            ex.draw(theme, self.focus == Focus::Explorer, &mut under, &mut text);
            let r = l.explorer.unwrap_or_default();
            under.push(Quad { x: r.x + r.w - 1.0, y: r.y, w: 1.0, h: r.h, color: theme.divider });
        }

        // Tab bar.
        under.push(Quad { x: l.tabs.x, y: l.tabs.y, w: l.tabs.w, h: l.tabs.h, color: theme.tab_bar });
        let accent_h = (2.0 * scale).round();
        for (i, &(x0, x1)) in self.tab_spans.iter().enumerate() {
            if i == self.active {
                under.push(Quad { x: x0, y: l.tabs.y, w: x1 - x0, h: l.tabs.h, color: theme.tab_active });
                under.push(Quad { x: x0, y: l.tabs.y, w: x1 - x0, h: accent_h, color: theme.accent });
            }
            under.push(Quad { x: x1, y: l.tabs.y, w: 1.0, h: l.tabs.h, color: theme.divider });
        }
        text.push(TextBlock {
            buffer: &self.tab_bar,
            x: l.tabs.x,
            y: l.tabs.y + (l.tabs.h - self.ui_font.line_h) / 2.0,
            color: theme.tab_text,
            clip: Some([l.tabs.x as i32, 0, (l.tabs.x + l.tabs.w) as i32, l.tabs.h as i32]),
        });

        self.tabs[self.active].draw(theme, scale, &mut under, &mut over, &mut text);

        // Status bar.
        let s = l.status;
        under.push(Quad { x: s.x, y: s.y, w: s.w, h: s.h, color: theme.status_bg });
        let pad = self.ui_font.cell_w;
        let text_y = s.y + (s.h - self.ui_font.line_h) / 2.0;
        let right_w = self.status_right.layout_runs().next().map_or(0.0, |r| r.line_w);
        let msg_color = if self.message.is_some() { theme.status_accent } else { theme.status_text };
        text.push(TextBlock {
            buffer: &self.status_left,
            x: pad,
            y: text_y,
            color: msg_color,
            clip: Some([0, s.y as i32, (s.w - right_w - 2.0 * pad) as i32, (s.y + s.h) as i32]),
        });
        text.push(TextBlock {
            buffer: &self.status_right,
            x: s.w - right_w - pad,
            y: text_y,
            color: theme.status_text,
            clip: None,
        });

        if popup_visible {
            self.popup.draw(theme, self.ui_font, s.w, s.y, &mut overlay, &mut overlay_text);
        }
        let window = (s.w, s.y + s.h);
        if let Some(c) = &self.completion {
            let ed = &self.tabs[self.active];
            let (x, y) = ed.screen_pos(c.anchor);
            let lh = ed.line_height();
            c.draw(theme, self.ui_font, x - self.ui_font.cell_w * 0.75, y + lh, y, window, scale, &mut overlay, &mut overlay_text);
        }
        if let Some(t) = &self.tooltip {
            t.draw(theme, self.ui_font, window, scale, &mut overlay, &mut overlay_text);
        }
        if matches!(self.overlay, Overlay::Palette) {
            let top = l.tabs.y + l.tabs.h * 0.5;
            self.palette.draw(theme, self.ui_font, s.w, top, scale, &mut overlay, &mut overlay_text);
        }

        let scene = Scene {
            background: theme.background,
            under: &under,
            text: &text,
            over: &over,
            overlay: &overlay,
            overlay_text: &overlay_text,
        };
        if self.renderer.draw(&scene) {
            self.latency.presented();
            if self.first_frame.is_none() {
                self.first_frame = Some(self.started.elapsed());
                log_stage("first frame", self.started);
                self.window.set_visible(true);
            }
        } else {
            log::debug!("frame skipped, retrying");
            self.window.request_redraw();
        }
    }
}
