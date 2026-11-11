// Release builds on Windows open no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Wyven – the editor app: window, input, and wiring of tabs, explorer, highlighting and
//! renderer.
//!
//! Start: `wyven <ordner>` opens a project, `wyven <datei>` opens the file and its folder.
//! Without an argument the most recently used project opens.

mod ai;
mod claude;
mod completion;
mod config;
mod debug;
mod dock;
mod diff_view;
mod editor;
mod explorer;
mod find;
mod git_panel;
mod help;
mod index;
mod keymap;
mod leader;
mod lsp;
mod markdown;
mod menu;
mod metrics;
mod palette;
mod popup;
mod search_panel;
mod settings;
mod settings_page;
mod terminal;
mod theme;

use std::collections::{HashMap, HashSet};
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
use wyven_buffer::{Document, Movement};
use wyven_render::{
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
    Terminal,
    Search,
    Git,
    Debug,
    Ai,
    Claude,
}

/// Startup profiling: logs elapsed time and RAM after each startup stage.
fn log_stage(stage: &str, started: Instant) {
    let ram = metrics::ram_mb().map_or("–".into(), |mb| format!("{mb:.0} MB"));
    log::info!("startup {stage}: {} · RAM {ram}", metrics::ms(Some(started.elapsed())));
}

/// Release builds have no console, so a crash would vanish without a trace: write the panic
/// message, where it happened and a backtrace to `%APPDATA%\Wyven\crash.log`.
fn install_crash_log() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let when = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let report = format!(
            "Wyven {} crashed (unix time {when})\n{info}\n\nBacktrace:\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
        if let Some(dir) = config::config_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("crash.log"), &report);
        }
        default_hook(info);
    }));
}

fn main() {
    config::migrate_old_config_dir();
    install_crash_log();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // wyven [folder or file] [--terminal] [--git] [--diff] [--goto=LINE] [--preview] [--settings]
    let mut path = None;
    let (mut open_terminal, mut open_git, mut open_diff) = (false, false, false);
    let mut goto_line: Option<usize> = None;
    let mut open_preview = false;
    let mut open_settings = false;
    for arg in std::env::args_os().skip(1) {
        if arg == "--terminal" {
            open_terminal = true;
        } else if arg == "--git" {
            open_git = true;
        } else if arg == "--diff" {
            open_diff = true;
        } else if arg == "--preview" {
            open_preview = true;
        } else if arg == "--settings" {
            open_settings = true;
        } else if let Some(n) = arg.to_str().and_then(|a| a.strip_prefix("--goto=")) {
            goto_line = n.parse().ok();
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        }
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App { path, open_terminal, open_git, open_diff, goto_line, open_preview, open_settings, state: None, proxy };
    event_loop.run_app(&mut app).expect("event loop error");
}

struct App {
    path: Option<PathBuf>,
    open_terminal: bool,
    open_git: bool,
    open_diff: bool,
    goto_line: Option<usize>,
    open_preview: bool,
    open_settings: bool,
    state: Option<State>,
    proxy: EventLoopProxy<UserEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Editor,
    Explorer,
    Terminal,
    Search,
    Git,
    Debug,
}

use dock::{Area, Panel};

/// A panel tab being dragged with the mouse.
struct PanelDrag {
    panel: Panel,
    from: (f32, f32),
    /// Far enough from the press to count as a drag (else it was a click).
    moved: bool,
    target: Option<(Area, usize)>,
}

enum Overlay {
    None,
    Leader { group: &'static str, since: Instant, shown: bool },
    Recent(Vec<PathBuf>),
    Palette,
    /// Quick actions to pick from.
    Menu(menu::Menu),
    /// New name for the symbol at `pos`.
    Rename { prompt: menu::Prompt, pos: usize },
    /// Branch picker (items: branch names, `null` = create a new branch).
    Branches(menu::Menu),
    /// Name for a new branch.
    NewBranch(menu::Prompt),
    /// F5 with several startable projects: which one.
    DebugProjects(menu::Menu),
    /// Alt → d → a: process to attach to.
    Processes(menu::Menu),
    /// Shift+F12 or a CodeLens click: places to jump to.
    References(menu::Menu),
    /// Alt → d → w: expression to watch.
    Watch(menu::Prompt),
    /// Alt → a → k: the Anthropic API key.
    ApiKey(menu::Prompt),
}

struct Mouse {
    x: f32,
    y: f32,
    dragging: bool,
    last_click: Option<(Instant, usize)>,
    /// Dragging the minimap box: mouse y and scroll line at the press.
    minimap_drag: Option<(f32, f32)>,
}

/// Screen layout, recomputed on resize, font change and when the explorer is toggled.
#[derive(Default, Clone, Copy)]
struct Layout {
    /// The dock areas (with their panel tab strip), when open.
    left: Option<Rect>,
    right: Option<Rect>,
    bottom: Option<Rect>,
    tabs: Rect,
    editor: Rect,
    /// Markdown preview, right of the editor.
    preview: Option<Rect>,
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
    /// Where the panels are docked and which ones show.
    dock: dock::Dock,
    dock_bar: dock::DockBar,
    panel_drag: Option<PanelDrag>,
    /// The dock area whose edge is being dragged to resize it.
    resizing: Option<Area>,
    /// The mouse cursor shape last set (arrow or resize arrows).
    cursor_resize: Option<Area>,
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
    /// AI switch in the status bar, and where it was drawn (for clicks).
    status_ai: Buffer,
    status_ai_span: (f32, f32),
    ai: ai::AiAssist,
    /// The Claude Code bridge (MCP server for Claude Code in a terminal).
    claude: Option<claude::Bridge>,
    /// Claude's proposed edit waiting for Ctrl+Enter / Esc.
    claude_diff: Option<claude::PendingDiff>,
    /// Last selection sent to Claude Code (file, anchor, head, version).
    claude_sel: Option<(PathBuf, usize, usize, u64)>,
    message: Option<(String, Instant)>,
    /// A destructive action waiting for confirmation by repeating it (close dirty tab/window).
    armed: Option<(&'static str, usize, Instant)>,

    proxy: EventLoopProxy<UserEvent>,
    /// One language server per language, started when the first file of it opens.
    servers: HashMap<lsp::ServerKind, lsp::LanguageServer>,
    /// Languages whose server could not start (not retried).
    lsp_failed: HashSet<lsp::ServerKind>,
    completion: Option<completion::Completion>,
    tooltip: Option<completion::Tooltip>,
    /// Where and since when the mouse rests (for hover info).
    hover_rest: Option<(Instant, f32, f32)>,

    settings: settings::Settings,
    settings_mtime: Option<std::time::SystemTime>,
    terminal: terminal::TerminalPanel,
    /// The shell was started once (a saved layout can show the terminal at startup).
    terminal_started: bool,
    find: find::FindBar,
    find_labels: find::ToggleLabels,
    search: search_panel::SearchPanel,
    git: git_panel::GitPanel,
    debugger: debug::Debugger,
    /// Breakpoints of files that are not open (open tabs keep their own).
    breakpoint_store: debug::Breakpoints,
    /// The project F5 starts (asked once if there are several).
    debug_project: Option<PathBuf>,
    /// Alt → m: the Markdown preview is wanted (shown while a Markdown file is active).
    preview_on: bool,
    preview: Option<markdown::Preview>,
    /// Font for the preview's normal text.
    prose_family: String,
    /// The pending F5 / Ctrl+F5 runs without the debugger.
    run_only: bool,
    /// Side-by-side diff shown instead of the editor.
    diff: Option<diff_view::DiffView>,
    settings_page: settings_page::SettingsPage,

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
            .with_title("Wyven")
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
        if self.open_terminal {
            state.toggle_terminal();
        }
        if self.open_git {
            state.git_command(leader::GitCmd::Panel);
        }
        if let Some(line) = self.goto_line {
            state.editor_mut().jump_to_line(line, None);
        }
        if self.open_settings {
            state.open_settings_page();
        }
        if self.open_preview {
            state.toggle_preview();
        }
        if self.open_diff {
            state.git_command(leader::GitCmd::Diff);
        }
        // Hidden windows get no redraw events, so draw the first frame now; it shows the window.
        state.redraw();
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => {
                if state.confirm_close_window() {
                    state.debugger.stop();
                    for server in state.servers.values_mut() {
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
                if settings::modified() != state.settings_mtime {
                    state.reload_settings();
                }
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
                if let Some(area) = state.resizing {
                    state.resize_area(area, x, y);
                }
                if state.panel_drag.is_some() {
                    state.panel_drag_move(x, y);
                }
                // Resize arrows over the edge of a dock area.
                let edge = state.resizing.or_else(|| state.splitter_at(x, y));
                if edge != state.cursor_resize {
                    state.cursor_resize = edge;
                    state.window.set_cursor(match edge {
                        Some(Area::Bottom) => winit::window::CursorIcon::RowResize,
                        Some(_) => winit::window::CursorIcon::ColResize,
                        None => winit::window::CursorIcon::Default,
                    });
                }
                state.hover_rest = (!state.mouse.dragging).then(|| (Instant::now(), x, y));
                if state.terminal.selecting {
                    let (row, col) = state.terminal.cell_at(x, y);
                    if let Some(t) = &state.terminal.term {
                        t.select_to(row, col);
                    }
                    state.terminal.dirty = true;
                    state.window.request_redraw();
                }
                if let Some((y0, start)) = state.mouse.minimap_drag {
                    state.editor_mut().minimap_drag(start, y - y0);
                    state.window.request_redraw();
                }
                if state.diff.is_none() {
                    let hover = state.editor().minimap_contains(x, y);
                    if hover != state.editor().minimap_hover {
                        state.editor_mut().minimap_hover = hover;
                        state.window.request_redraw();
                    }
                    let gutter = state.editor().gutter_contains(x, y);
                    if gutter != state.editor().gutter_hover {
                        state.editor_mut().gutter_hover = gutter;
                        state.window.request_redraw();
                    }
                }
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

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        if let Some(state) = &mut self.state {
            match event {
                UserEvent::Lsp => state.lsp_poll(),
                UserEvent::Terminal => state.terminal_poll(),
                UserEvent::Search => {
                    state.search.poll();
                    state.window.request_redraw();
                }
                UserEvent::Git => state.git_poll(),
                UserEvent::Debug => state.debug_poll(),
                UserEvent::Ai => state.ai_poll(),
                UserEvent::Claude => state.claude_poll(),
            }
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
            state.reload_changed_files();
            state.git_tick(now);
            state.window.request_redraw();
        }
        let mut wake = state.next_stats;
        if let Some(t) = state.lsp_tick(now) {
            wake = wake.min(t);
        }
        if let Some(due) = state.ai.due {
            if now >= due {
                state.ai_trigger();
            } else {
                wake = wake.min(due);
            }
        }
        let proxy = state.proxy.clone();
        let was_due = state.search.due.is_some();
        match state.search.start_if_due(now, move || {
            let _ = proxy.send_event(UserEvent::Search);
        }) {
            Some(t) => wake = wake.min(t),
            None if was_due => state.window.request_redraw(),
            None => {}
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

fn font_metrics(fs: &mut FontSystem, family: &str, size: f32, line_height: f32) -> FontMetrics {
    let line_h = (size * line_height).round();
    let attrs = Attrs::new().family(Family::Name(family));
    let cell_w = measure_cell_width(fs, Metrics::new(size, line_h), &attrs);
    FontMetrics { size, line_h, cell_w }
}

/// The font from the settings if it is installed, otherwise the best installed monospace font.
fn choose_family(fs: &FontSystem, wanted: &str) -> String {
    let wanted = wanted.trim();
    if !wanted.is_empty() {
        let installed = fs
            .db()
            .faces()
            .any(|f| f.families.iter().any(|(name, _)| name.eq_ignore_ascii_case(wanted)));
        if installed {
            return wanted.to_string();
        }
        log::warn!("font {wanted} is not installed");
    }
    pick_monospace_family(fs).unwrap_or_else(|| "monospace".into())
}

/// The expression under the mouse for debugger hovers: a name with its `.` chain to the
/// left (`player.Position.X` when pointing at `X`).
fn expression_at(rope: &wyven_buffer::Rope, pos: usize) -> Option<String> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let len = rope.len_chars();
    if pos >= len || !word(rope.char(pos)) {
        return None;
    }
    let mut end = pos;
    while end < len && word(rope.char(end)) {
        end += 1;
    }
    let mut start = pos;
    while start > 0 && (word(rope.char(start - 1)) || rope.char(start - 1) == '.') {
        start -= 1;
    }
    let expr = rope.slice(start..end).to_string();
    let expr = expr.trim_start_matches('.');
    (!expr.is_empty()).then(|| expr.to_string())
}

/// The language server for a tab's language, if one runs.
fn server_of<'a>(
    servers: &'a mut HashMap<lsp::ServerKind, lsp::LanguageServer>,
    tab: &EditorView,
) -> Option<&'a mut lsp::LanguageServer> {
    tab.lsp_kind().and_then(|k| servers.get_mut(&k))
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
        let (settings, settings_error) = match settings::load() {
            Ok(s) => (s, None),
            Err(e) => (settings::Settings::default(), Some(e)),
        };
        let fs = renderer.font_system();
        let family = choose_family(fs, &settings.font.family);
        log::info!("font: {family}");
        let (size, lh) = (settings.font.size, settings.font.line_height);
        let font = font_metrics(fs, &family, size * scale, lh);
        let ui_font = font_metrics(fs, &family, size * scale * settings.ui.font_scale, lh);
        let editor = EditorView::new(Document::default(), fs, font, family.clone());
        let ui_metrics = Metrics::new(ui_font.size, ui_font.line_h);
        let mut line_buffer = || {
            let mut b = Buffer::new(fs, ui_metrics);
            b.set_wrap(Wrap::None);
            b
        };
        let (status_left, status_right, tab_bar) = (line_buffer(), line_buffer(), line_buffer());
        let status_ai = line_buffer();
        let mut ai = ai::AiAssist::new(settings.ai.enabled, ai::backend_from(&settings.ai), settings.ai.delay_ms);
        let ai_proxy = proxy.clone();
        ai.set_wake(move || {
            let _ = ai_proxy.send_event(UserEvent::Ai);
        });
        let popup = Popup::new(fs, ui_font);
        let palette = Palette::new(fs, ui_font);
        let terminal_panel = terminal::TerminalPanel::new(fs, font, ui_font, family.clone());
        let dock_bar = dock::DockBar::new(fs, ui_font, family.clone());
        let settings_page = settings_page::SettingsPage::new(ui_font, family.clone());
        let find_bar = find::FindBar::new(fs, ui_font);
        let initial_theme = Theme::by_name(&settings.theme).unwrap_or_else(Theme::dark);
        let find_labels = find::ToggleLabels::new(fs, ui_font, &family, &initial_theme);
        let search = search_panel::SearchPanel::new(fs, ui_font, family.clone());
        let mut git = git_panel::GitPanel::new(fs, ui_font, family.clone());
        let mut debugger = debug::Debugger::new(fs, ui_font, family.clone());
        let debug_proxy = proxy.clone();
        debugger.set_wake(move || {
            let _ = debug_proxy.send_event(UserEvent::Debug);
        });
        let git_proxy = proxy.clone();
        git.set_wake(move || {
            let _ = git_proxy.send_event(UserEvent::Git);
        });

        let mut state = Self {
            window,
            renderer,
            theme: Theme::by_name(&settings.theme).unwrap_or_else(Theme::dark),
            family,
            font_pt: settings.font.size,
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
            dock: dock::Dock::load(),
            dock_bar,
            panel_drag: None,
            resizing: None,
            cursor_resize: None,
            focus: Focus::Editor,
            overlay: Overlay::None,
            popup,
            palette,
            index: FileIndex::default(),
            keymap: Keymap::default(),
            modifiers: ModifiersState::empty(),
            alt_alone: false,
            mouse: Mouse { x: 0.0, y: 0.0, dragging: false, last_click: None, minimap_drag: None },
            clipboard: arboard::Clipboard::new().ok(),
            last_copy: None,
            status_left,
            status_right,
            status_ai,
            status_ai_span: (0.0, 0.0),
            ai,
            claude: None,
            claude_diff: None,
            claude_sel: None,
            message: None,
            armed: None,
            settings,
            settings_mtime: settings::modified(),
            terminal: terminal_panel,
            terminal_started: false,
            find: find_bar,
            find_labels,
            search,
            git,
            debugger,
            breakpoint_store: debug::Breakpoints::new(),
            debug_project: None,
            preview_on: false,
            preview: None,
            prose_family: String::new(),
            run_only: false,
            diff: None,
            settings_page,
            proxy,
            servers: HashMap::new(),
            lsp_failed: HashSet::new(),
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
        state.start_claude_bridge();
        if let Some(e) = settings_error {
            state.show(format!("settings.toml: {e}"));
        }
        let first = std::mem::take(&mut state.tabs);
        state.tabs = first.into_iter().map(|mut t| { state.configure_view(&mut t); t }).collect();
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
        let body_h = h - status_h;
        let cell = self.ui_font.cell_w;
        let side_w = |me: &Self, a: Area| -> f32 {
            if !me.area_open(a) {
                return 0.0;
            }
            let cells = me.dock.area(a).size.unwrap_or(me.settings.ui.explorer_width);
            (cells * cell).clamp(cell * 12.0, w * 0.45).round()
        };
        let left_w = side_w(self, Area::Left);
        let right_w = side_w(self, Area::Right).min(w - left_w - cell * 20.0).max(0.0);
        let center_w = w - left_w - right_w;
        let bottom_h = if self.area_open(Area::Bottom) {
            let pct = self.dock.area(Area::Bottom).size.unwrap_or(self.settings.terminal.height);
            (body_h * pct / 100.0)
                .round()
                .max(self.font.line_h * 4.0)
                .min(body_h - tabs_h - self.font.line_h * 3.0)
        } else {
            0.0
        };
        let editor_h = body_h - tabs_h - bottom_h;
        // The Markdown preview takes the right half of the editor.
        let preview_w = if self.preview_visible() { (center_w / 2.0).round() } else { 0.0 };
        self.layout = Layout {
            left: (left_w > 0.0).then_some(Rect { x: 0.0, y: 0.0, w: left_w, h: body_h }),
            right: (right_w > 0.0).then_some(Rect { x: w - right_w, y: 0.0, w: right_w, h: body_h }),
            bottom: (bottom_h > 0.0).then_some(Rect { x: left_w, y: body_h - bottom_h, w: center_w, h: bottom_h }),
            tabs: Rect { x: left_w, y: 0.0, w: center_w, h: tabs_h },
            editor: Rect { x: left_w, y: tabs_h, w: center_w - preview_w, h: editor_h },
            preview: (preview_w > 0.0)
                .then_some(Rect { x: left_w + center_w - preview_w, y: tabs_h, w: preview_w, h: editor_h }),
            status: Rect { x: 0.0, y: body_h, w, h: status_h },
        };
        // Each panel draws below the tab strip of its area.
        for p in Panel::ALL {
            let Some(r) = self.layout_area(self.dock.area_of(p)) else { continue };
            let c = dock::content(r, self.ui_font);
            match p {
                Panel::Explorer => {
                    if let Some(ex) = &mut self.explorer {
                        ex.set_area(c);
                    }
                }
                Panel::Search => self.search.set_area(c),
                Panel::Git => self.git.set_area(c),
                Panel::Debug => self.debugger.set_area(c),
                Panel::Terminal => self.terminal.set_area(c),
            }
        }
        let strips: Vec<(Area, Rect, Vec<Panel>)> = Area::ALL
            .into_iter()
            .filter_map(|a| self.layout_area(a).map(|r| (a, r, self.dock.area(a).panels.clone())))
            .collect();
        self.dock_bar.place(&strips);
        let area = self.layout.editor;
        let scale = self.renderer.scale_factor();
        for t in &mut self.tabs {
            t.set_area(area);
            t.scale = scale;
        }
        if let Some(d) = &mut self.diff {
            d.set_area(area);
        }
        self.settings_page.set_area(area);
        if let (Some(p), Some(r)) = (&mut self.preview, self.layout.preview) {
            p.set_area(r);
        }
        self.window.request_redraw();
    }

    fn layout_area(&self, a: Area) -> Option<Rect> {
        match a {
            Area::Left => self.layout.left,
            Area::Right => self.layout.right,
            Area::Bottom => self.layout.bottom,
        }
    }

    /// The Explorer has nothing to show without an open folder.
    fn panel_available(&self, p: Panel) -> bool {
        p != Panel::Explorer || self.explorer.is_some()
    }

    fn area_open(&self, a: Area) -> bool {
        let s = self.dock.area(a);
        s.visible && s.active.is_some_and(|p| self.panel_available(p))
    }

    /// The panel is on screen.
    fn panel_shown(&self, p: Panel) -> bool {
        self.dock.shown(p) && self.layout_area(self.dock.area_of(p)).is_some()
    }

    fn focused_panel(&self) -> Option<Panel> {
        match self.focus {
            Focus::Explorer => Some(Panel::Explorer),
            Focus::Search => Some(Panel::Search),
            Focus::Git => Some(Panel::Git),
            Focus::Debug => Some(Panel::Debug),
            Focus::Terminal => Some(Panel::Terminal),
            Focus::Editor => None,
        }
    }

    /// Saves the panel layout and lays the window out again.
    fn dock_changed(&mut self) {
        self.dock.save();
        self.relayout();
    }

    /// Shows a panel and moves the focus there (click on its tab).
    fn activate_panel(&mut self, p: Panel) {
        match p {
            Panel::Explorer => {
                if self.explorer.is_none() {
                    self.show("No folder open: Alt → p → o opens one.");
                    return;
                }
                self.dock.show(p);
                self.focus = Focus::Explorer;
            }
            Panel::Search => {
                if self.search.has_root() {
                    self.dock.show(p);
                    self.focus = Focus::Search;
                } else {
                    self.open_search_panel(false);
                }
            }
            Panel::Git => self.show_git_panel(),
            Panel::Debug => self.show_debug_panel(true),
            Panel::Terminal => {
                self.dock.show(p);
                self.focus = Focus::Terminal;
                if self.terminal.term.is_none() {
                    self.relayout();
                    self.start_terminal();
                }
            }
        }
        self.dock_changed();
    }

    /// Alt → w → r: everything back to where it was at first.
    fn reset_layout(&mut self) {
        self.dock = dock::Dock::default();
        if self.focused_panel().is_some_and(|p| !self.dock.shown(p)) {
            self.focus = Focus::Editor;
        }
        self.dock_changed();
        self.show("Panel layout reset");
    }

    /// The dock area whose inner edge is under the mouse (to resize it).
    fn splitter_at(&self, x: f32, y: f32) -> Option<Area> {
        let grab = 4.0 * self.renderer.scale_factor();
        let near = |a: f32, b: f32| (a - b).abs() <= grab;
        if let Some(r) = self.layout.left
            && near(x, r.x + r.w)
            && y >= r.y
            && y < r.y + r.h
        {
            return Some(Area::Left);
        }
        if let Some(r) = self.layout.right
            && near(x, r.x)
            && y >= r.y
            && y < r.y + r.h
        {
            return Some(Area::Right);
        }
        if let Some(r) = self.layout.bottom
            && near(y, r.y)
            && x >= r.x
            && x < r.x + r.w
        {
            return Some(Area::Bottom);
        }
        None
    }

    /// Dragging an area's edge: sides in UI cells, the bottom in percent of the window.
    fn resize_area(&mut self, a: Area, x: f32, y: f32) {
        let (w, _) = self.renderer.size();
        let w = w as f32;
        let body_h = self.layout.status.y;
        let cell = self.ui_font.cell_w;
        let size = match a {
            Area::Left => (x.clamp(cell * 12.0, w * 0.45)) / cell,
            Area::Right => ((w - x).clamp(cell * 12.0, w * 0.45)) / cell,
            Area::Bottom => ((body_h - y) / body_h * 100.0).clamp(10.0, 80.0),
        };
        self.dock.area_mut(a).size = Some((size * 10.0).round() / 10.0);
        self.relayout();
    }

    /// Mouse moves while a panel tab is held: start the drag, follow the drop target.
    fn panel_drag_move(&mut self, x: f32, y: f32) {
        let strips: Vec<(Area, Rect)> = Area::ALL
            .into_iter()
            .filter_map(|a| {
                self.layout_area(a).map(|r| (a, Rect { x: r.x, y: r.y, w: r.w, h: dock::strip_h(self.ui_font) }))
            })
            .collect();
        let body = Rect { x: 0.0, y: 0.0, w: self.layout.status.w, h: self.layout.status.y };
        let Some(d) = &mut self.panel_drag else { return };
        let dist = ((x - d.from.0).powi(2) + (y - d.from.1).powi(2)).sqrt();
        if !d.moved && dist < 6.0 * self.renderer.scale_factor() {
            return;
        }
        d.moved = true;
        d.target = dock::drop_target(x, y, body, &self.dock_bar.spans, &strips);
        self.window.request_redraw();
    }

    /// Mouse released after dragging a panel tab.
    fn panel_drop(&mut self) {
        let Some(d) = self.panel_drag.take() else { return };
        if !d.moved {
            return;
        }
        if let Some((area, index)) = d.target {
            self.dock.move_panel(d.panel, area, index);
            self.dock_changed();
            self.activate_panel(d.panel);
        }
        self.window.request_redraw();
    }

    /// The preview shows while it is switched on and the active tab is a Markdown file.
    fn preview_visible(&self) -> bool {
        self.preview_on
            && self.diff.is_none()
            && self.tabs.get(self.active).is_some_and(|t| t.language_name() == "Markdown")
    }

    /// Alt → m.
    fn toggle_preview(&mut self) {
        if self.editor().language_name() != "Markdown" {
            self.show("The Markdown preview works for .md files.");
            return;
        }
        self.preview_on = !self.preview_on;
        if !self.preview_on {
            self.preview = None;
        }
        self.relayout();
    }

    /// Keeps the preview in step with the active tab: same file, current text, same place.
    fn prepare_preview(&mut self) {
        let Some(area) = self.layout.preview else {
            self.preview = None;
            return;
        };
        let ed = &self.tabs[self.active];
        let path = ed.doc.path().map(Path::to_path_buf).unwrap_or_default();
        if self.preview.as_ref().is_none_or(|p| p.path() != path.as_path()) {
            if self.prose_family.is_empty() {
                let fs = self.renderer.font_system();
                let segoe = fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == "Segoe UI"));
                self.prose_family = if segoe { "Segoe UI".into() } else { self.family.clone() };
            }
            let (size, lh) = (self.font.size, self.font.line_h);
            let fs = self.renderer.font_system();
            self.preview = Some(markdown::Preview::new(fs, path, size, lh, self.prose_family.clone(), self.family.clone()));
        }
        let ed = &self.tabs[self.active];
        let (text, version, top) = (ed.doc.text().to_string(), ed.doc.version(), ed.scroll_line);
        let (font, theme) = (self.font, &self.theme);
        let fs = self.renderer.font_system();
        if let Some(p) = &mut self.preview {
            p.set_area(area);
            p.set_font(font.size, font.line_h);
            p.prepare(fs, theme, &text, version);
            p.follow(top);
        }
    }

    /// Re-measures the fonts (zoom or DPI change).
    fn apply_font(&mut self) {
        let scale = self.renderer.scale_factor();
        let family = self.family.clone();
        let fs = self.renderer.font_system();
        let lh = self.settings.font.line_height;
        self.font = font_metrics(fs, &family, self.font_pt * scale, lh);
        self.ui_font = font_metrics(fs, &family, self.font_pt * scale * self.settings.ui.font_scale, lh);
        let (font, ui) = (self.font, self.ui_font);
        for t in &mut self.tabs {
            t.set_font(self.renderer.font_system(), font);
        }
        if let Some(ex) = &mut self.explorer {
            ex.set_font(ui);
        }
        self.popup.set_font(ui);
        self.palette.set_font(ui);
        self.terminal.set_font(font, ui);
        self.find.set_font(ui);
        self.find_labels.set_font(ui);
        self.search.set_font(ui);
        self.git.set_font(ui);
        self.debugger.set_font(ui);
        self.settings_page.set_font(ui, self.family.clone());
        self.dock_bar.set_font(self.renderer.font_system(), ui);
        // The diff view keeps its own fonts; reopening is cheaper than re-measuring it.
        self.diff = None;
        let m = Metrics::new(ui.size, ui.line_h);
        for b in [&mut self.status_left, &mut self.status_right, &mut self.tab_bar, &mut self.status_ai] {
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
        self.dock.show(Panel::Explorer);
        config::remember_project(&path);
        self.index.rebuild(&path);
        self.git.open_repo(&path);
        for t in &mut self.tabs {
            t.git_base = None;
        }
        self.load_git_heads();
        self.claude_workspace_changed();
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
                        self.show(format!("{} cannot be opened: {e}", file_name(path)));
                        return;
                    }
                }
            } else {
                let mut d = Document::default();
                if let Err(e) = d.save_as(path.to_path_buf()) {
                    self.show(format!("{} cannot be created: {e}", file_name(path)));
                    return;
                }
                d
            };
            let fs = self.renderer.font_system();
            let mut view = EditorView::new(doc, fs, self.font, self.family.clone());
            view.set_area(self.layout.editor);
            self.configure_view(&mut view);
            self.git.load_head(path);
            if let Some(lines) = self.breakpoint_store.get(path) {
                view.set_breakpoints(lines.clone());
            }
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
        self.diff = None;
        self.lsp_open_all();
    }

    fn activate(&mut self, index: usize) {
        self.active = index;
        self.diff = None;
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
        self.configure_view(&mut view);
        self.tabs.push(view);
        self.mru.push(self.tabs.len() - 1);
        self.activate(self.tabs.len() - 1);
        self.focus = Focus::Editor;
    }

    fn close_tab(&mut self, index: usize) {
        let tab = &self.tabs[index];
        if tab.doc.is_dirty() {
            let msg = format!("{} is unsaved! Ctrl+W again discards it", tab.file_name());
            if !self.confirm("close-tab", index, &msg) {
                return;
            }
        }
        let tab = self.tabs.remove(index);
        if let Some(p) = tab.doc.path() {
            if tab.breakpoints().is_empty() {
                self.breakpoint_store.remove(p);
            } else {
                self.breakpoint_store.insert(p.to_path_buf(), tab.breakpoints().to_vec());
            }
            self.closed.push(p.to_path_buf());
            for server in self.servers.values_mut() {
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
            self.configure_view(&mut view);
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
            self.show(format!("{kept} file(s) with unsaved changes stay open."));
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
                ed.refresh_disk_mtime();
                let name = ed.file_name();
                if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
                    server.saved(path);
                }
                let is_settings = ed.doc.path().is_some() && ed.doc.path() == settings::path().as_deref();
                self.show(format!("{name} saved"));
                if is_settings {
                    self.reload_settings();
                }
                if self.git.repo.is_some() {
                    self.git.refresh_status();
                }
            }
            Err(e) => self.show(format!("Save failed: {e}")),
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
            .set_title("Save As")
            .set_file_name(self.editor().file_name());
        if let Some(dir) = self.dialog_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(path) = dialog.save_file() else { return };
        match self.editor_mut().save_as(path.clone()) {
            Ok(()) => {
                self.show(format!("{} saved", file_name(&path)));
                self.git.load_head(&path);
                self.lsp_open_all();
                if let Some(ex) = &mut self.explorer {
                    ex.refresh();
                    ex.reveal(&path);
                }
            }
            Err(e) => self.show(format!("Save failed: {e}")),
        }
    }

    fn pick_file(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Open File");
        if let Some(dir) = self.dialog_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_path(&path);
        }
    }

    fn pick_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Open Folder");
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
        let msg = format!("{dirty} file(s) unsaved! Close again to discard");
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
            Overlay::Menu(_) => {
                self.menu_key(&key);
                return;
            }
            Overlay::Rename { .. } => {
                self.rename_key(event, &key);
                return;
            }
            Overlay::Branches(_) => {
                self.branch_menu_key(&key);
                return;
            }
            Overlay::References(_) => {
                self.references_key(&key);
                return;
            }
            Overlay::NewBranch(_) => {
                self.new_branch_key(event, &key);
                return;
            }
            Overlay::DebugProjects(_) | Overlay::Processes(_) => {
                self.debug_menu_key(&key);
                return;
            }
            Overlay::ApiKey(_) => {
                self.api_key_key(event, &key);
                return;
            }
            Overlay::Watch(_) => {
                self.watch_key(event, &key);
                return;
            }
            Overlay::None => {}
        }

        let mods = self.modifiers;
        self.tooltip = None;
        if self.focus == Focus::Terminal && self.panel_shown(Panel::Terminal) {
            self.terminal_key(event);
            return;
        }
        if self.focus == Focus::Search && self.panel_shown(Panel::Search) {
            self.search_key(event);
            return;
        }
        if self.focus == Focus::Git && self.panel_shown(Panel::Git) {
            self.git_key(event);
            return;
        }
        // F-keys (step, continue) work everywhere, also with the debug panel focused.
        let is_fkey = matches!(key, Key::Named(NamedKey::F5 | NamedKey::F9 | NamedKey::F10 | NamedKey::F11));
        if self.focus == Focus::Debug && self.panel_shown(Panel::Debug) && !is_fkey {
            let action = self.debugger.key(event);
            self.apply_debug_panel_action(action);
            return;
        }
        if self.diff.is_some() && self.focus == Focus::Editor {
            self.diff_key(event);
            return;
        }
        if self.focus == Focus::Editor
            && self.editor().settings_page
            && !(mods.control_key() && key == Key::Named(NamedKey::Tab))
        {
            self.settings_key(event);
            return;
        }
        if self.focus == Focus::Editor && self.find.open && self.find.focused {
            self.find_key(event);
            return;
        }
        if self.focus == Focus::Editor && !mods.control_key() && !mods.alt_key()
            && self.completion_key(&key) {
                return;
            }
        if self.focus == Focus::Editor && self.completion.is_none() && self.ghost_key(&key) {
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
        if let Some(first) = self.keymap.chord_pending() {
            self.show(if first == 'm' {
                "Ctrl+M … (M = fold/unfold, L = all, O = definitions, P = expand all)"
            } else {
                "Ctrl+K … (C = comment, U = uncomment, D = format, I = info)"
            });
            return;
        }
        // The chord hint has done its job once the second key arrived.
        if self.message.as_ref().is_some_and(|(m, _)| m.starts_with("Ctrl+K …") || m.starts_with("Ctrl+M …")) {
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

    /// Starts the language server of each language with an open file (the first time), and
    /// tells the servers about open files.
    fn lsp_open_all(&mut self) {
        let needed: HashSet<lsp::ServerKind> =
            self.tabs.iter().filter(|t| t.doc.path().is_some()).filter_map(|t| t.lsp_kind()).collect();
        for kind in needed {
            if self.servers.contains_key(&kind) || self.lsp_failed.contains(&kind) {
                continue;
            }
            let root = self
                .project_root()
                .map(Path::to_path_buf)
                .or_else(|| self.editor().doc.path().and_then(Path::parent).map(Path::to_path_buf));
            let Some(root) = root else { continue };
            let proxy = self.proxy.clone();
            match lsp::LanguageServer::start(kind, &root, move || {
                let _ = proxy.send_event(UserEvent::Lsp);
            }) {
                Ok(mut server) => {
                    server.want_inlay_hints = self.settings.editor.inlay_hints;
                    server.want_code_lens = self.settings.editor.code_lens;
                    self.servers.insert(kind, server);
                }
                Err(e) => {
                    self.lsp_failed.insert(kind);
                    self.show(e);
                }
            }
        }
        for t in &self.tabs {
            let Some(path) = t.doc.path() else { continue };
            if let Some(server) = server_of(&mut self.servers, t)
                && !server.is_open(path)
            {
                server.open(path, t.doc.text().to_string(), t.doc.version());
            }
        }
    }

    /// Sends the active document right away (before asking the server something about it).
    fn lsp_sync_active(&mut self) -> bool {
        let ed = &self.tabs[self.active];
        let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) else { return false };
        if !server.is_open(path) {
            return false;
        }
        server.sync(path, ed.doc.text(), ed.doc.version());
        true
    }

    fn lsp_after_edit(&mut self) {
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
            server.changed(path, ed.doc.version());
        }
    }

    /// The active tab's language server has loaded and answers requests.
    fn lsp_ready(&self) -> bool {
        self.editor()
            .lsp_kind()
            .and_then(|k| self.servers.get(&k))
            .is_some_and(|s| s.status == lsp::Status::Ready)
    }

    /// Opens, filters or closes the completion list after a command.
    fn update_completion(&mut self, typed: Option<&str>, trigger: Option<Option<char>>) {
        let ed = &self.tabs[self.active];
        if ed.lsp_kind().is_none() || ed.doc.path().is_none() || ed.doc.selection().len() > 1 {
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
            (None, Some(".")) if self.settings.editor.auto_complete => Some(Some('.')),
            (None, Some(t)) => {
                let mut chars = t.chars();
                let ident = matches!((chars.next(), chars.next()), (Some(ch), None) if ch.is_alphabetic() || ch == '_');
                (ident && ed.word_start() + 1 == head && self.settings.editor.auto_complete).then_some(None)
            }
            _ => None,
        };
        let Some(trigger) = trigger else { return };
        if !self.lsp_ready() {
            if typed.is_none() {
                self.show("The language server is still loading the project …");
            }
            return;
        }
        let anchor = self.editor().word_start();
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
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
        if self.tabs[self.active].lsp_kind().is_none() {
            self.show("Go to definition needs a language server (C#, TypeScript, HTML, CSS, JSON).");
            return;
        }
        if !self.lsp_ready() {
            self.show("The language server is still loading the project …");
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
            server.definition(path, ed.doc.text(), pos);
        }
    }

    fn request_hover(&mut self, pos: usize, x: f32, y: f32) {
        if self.tabs[self.active].lsp_kind().is_none() || !self.lsp_ready() {
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
            server.hover(path, ed.doc.text(), pos, x, y);
        }
    }

    fn request_format(&mut self) {
        if self.tabs[self.active].lsp_kind().is_none() {
            self.show("Formatting needs a language server (C#, TypeScript, HTML, CSS, JSON).");
            return;
        }
        if !self.lsp_ready() {
            self.show("The language server is still loading the project …");
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        let spaces = ed.doc.indent_unit != "\t";
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
            server.format(path, ed.doc.version(), ed.doc.tab_width, spaces);
        }
    }

    /// Handles everything the language servers sent.
    fn lsp_poll(&mut self) {
        let events: Vec<lsp::Event> = self.servers.values_mut().flat_map(|s| s.poll()).collect();
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
                        let pos = wyven_lsp::from_lsp(ed.doc.text(), &start);
                        ed.jump_to(pos);
                    }
                }
                lsp::Event::DefinitionNotFound => self.show("No definition found"),
                lsp::Event::WorkspaceEdit { edit, what } => self.apply_workspace_edit(&edit, &what),
                lsp::Event::CodeActions(actions) => self.show_actions("Quick actions (digit or Enter, Esc)", actions),
                lsp::Event::InlayHints { path, version, hints } => {
                    if let Some(i) = self.tab_for(&path)
                        && self.tabs[i].doc.version() == version
                    {
                        let rope = self.tabs[i].doc.text();
                        let hints = hints
                            .iter()
                            .map(|(pos, label, pad_left, pad_right)| {
                                let p = wyven_lsp::from_lsp(rope, pos);
                                let line = rope.char_to_line(p);
                                editor::InlayHint {
                                    line,
                                    col: p - rope.line_to_char(line),
                                    label: label.clone(),
                                    pad_left: *pad_left,
                                    pad_right: *pad_right,
                                }
                            })
                            .collect();
                        self.tabs[i].set_inlay_hints(hints);
                    }
                }
                lsp::Event::CodeLens { path, version, lenses } => {
                    if let Some(i) = self.tab_for(&path)
                        && self.tabs[i].doc.version() == version
                    {
                        let rope = self.tabs[i].doc.text();
                        let lenses = lenses
                            .iter()
                            .map(|(start, title)| {
                                let p = wyven_lsp::from_lsp(rope, start);
                                let line = rope.char_to_line(p);
                                editor::CodeLens { line, col: p - rope.line_to_char(line), title: title.clone() }
                            })
                            .collect();
                        self.tabs[i].set_lenses(lenses);
                    }
                }
                lsp::Event::References(refs) => self.show_references(refs),
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
                        .map(|(s, e, text)| wyven_buffer::Change {
                            start: wyven_lsp::from_lsp(rope, s),
                            end: wyven_lsp::from_lsp(rope, e),
                            text: text.replace("\r\n", "\n"),
                        })
                        .collect::<Vec<_>>();
                    let n = changes.len();
                    ed.doc.apply_changes(changes);
                    ed.after_edit();
                    let version = self.tabs[i].doc.version();
                    for server in self.servers.values_mut() {
                        server.changed(&path, version);
                    }
                    self.show(if n == 0 { "Already formatted".to_string() } else { "Formatted".to_string() });
                }
            }
        }
        self.window.request_redraw();
    }

    /// Converts the server's problem list for a file into ranges in its tab.
    fn apply_diagnostics(&mut self, path: &Path) {
        let list = self.servers.values().find_map(|s| s.diagnostics.get(path).cloned()).unwrap_or_default();
        if let Some(t) = self.tabs.iter_mut().find(|t| t.doc.path() == Some(path)) {
            let rope = t.doc.text();
            t.diagnostics = list
                .iter()
                .filter(|d| d.severity <= 3)
                .map(|d| editor::DiagSpan {
                    start: wyven_lsp::from_lsp(rope, &d.start),
                    end: wyven_lsp::from_lsp(rope, &d.end),
                    severity: d.severity,
                    raw: d.raw.clone(),
                    message: if d.code.is_empty() { d.message.clone() } else { format!("{}: {}", d.code, d.message) },
                })
                .collect();
        }
    }

    /// Periodic work for the language server: send pending changes, ask for diagnostics,
    /// show hover info when the mouse rests. Returns when to wake up next.
    fn lsp_tick(&mut self, now: Instant) -> Option<Instant> {
        let mut wake: Option<Instant> = None;
        for server in self.servers.values_mut() {
            for path in server.due_syncs(now) {
                if let Some(t) = self.tabs.iter().find(|t| t.doc.path() == Some(path.as_path())) {
                    server.sync(&path, t.doc.text(), t.doc.version());
                }
            }
            if let Some(t) = server.tick(now) {
                wake = Some(wake.map_or(t, |w| w.min(t)));
            }
        }
        if let Some((since, x, y)) = self.hover_rest {
            let due = since + HOVER_DELAY;
            if now >= due {
                self.hover_rest = None;
                if self.settings.editor.hover
                    && self.tooltip.is_none()
                    && self.editor().contains(x, y)
                    && !self.editor().minimap_contains(x, y)
                    && matches!(self.overlay, Overlay::None)
                {
                    let pos = self.editor().hit_test(x, y);
                    let rope = self.editor().doc.text();
                    let on_word = pos < rope.len_chars() && {
                        let c = rope.char(pos);
                        c.is_alphanumeric() || c == '_'
                    };
                    if on_word && self.debugger.phase == debug::Phase::Stopped {
                        if let Some(expr) = expression_at(rope, pos) {
                            self.debugger.hover(expr, x, y);
                        }
                    } else if on_word {
                        self.request_hover(pos, x, y);
                    }
                }
            } else {
                wake = Some(wake.map_or(due, |w: Instant| w.min(due)));
            }
        }
        wake
    }

    // ----- rename, quick actions, workspace edits -----

    /// F2: asks for the new name of the symbol under the cursor.
    fn start_rename(&mut self) {
        if self.tabs[self.active].lsp_kind().is_none() {
            self.show("Rename needs a language server (C#, TypeScript, HTML, CSS, JSON).");
            return;
        }
        if !self.lsp_ready() {
            self.show("The language server is still loading the project …");
            return;
        }
        let ed = self.editor();
        let pos = ed.doc.selection().primary().head;
        let start = ed.word_start();
        let rope = ed.doc.text();
        let mut end = pos;
        while end < rope.len_chars() && {
            let c = rope.char(end);
            c.is_alphanumeric() || c == '_'
        } {
            end += 1;
        }
        if start == end {
            self.show("Put the cursor on a name, then press F2");
            return;
        }
        let name = rope.slice(start..end).to_string();
        let fs = self.renderer.font_system();
        let prompt = menu::Prompt::new(fs, self.ui_font, &format!("Rename “{name}” to … (Enter / Esc)"), &name);
        self.overlay = Overlay::Rename { prompt, pos: start };
    }

    fn rename_key(&mut self, event: &KeyEvent, key: &Key) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        let Overlay::Rename { prompt, pos } = &mut self.overlay else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.overlay = Overlay::None,
            Key::Named(NamedKey::Enter) => {
                let (name, pos) = (prompt.text.trim().to_string(), *pos);
                self.overlay = Overlay::None;
                let valid = !name.is_empty()
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '@');
                if !valid {
                    self.show("Invalid name");
                    return;
                }
                self.lsp_sync_active();
                let ed = &self.tabs[self.active];
                if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
                    server.rename(path, ed.doc.text(), pos, &name);
                }
            }
            Key::Named(NamedKey::Backspace) => prompt.backspace(ctrl),
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::End | NamedKey::Home) => prompt.keep(),
            Key::Character(c) if ctrl && c.as_str() == "v" => {
                if let Some(t) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok())
                    && let Overlay::Rename { prompt, .. } = &mut self.overlay {
                        prompt.type_text(t.lines().next().unwrap_or("").trim());
                    }
            }
            _ => {
                if let Some(t) = Keymap::text(event, mods) {
                    prompt.type_text(&t);
                }
            }
        }
    }

    /// Strg+.: asks the server what can be done at the cursor or selection.
    fn request_quick_actions(&mut self) {
        if self.tabs[self.active].lsp_kind().is_none() {
            self.show("Quick actions need a language server (C#, TypeScript, HTML, CSS, JSON).");
            return;
        }
        if !self.lsp_ready() {
            self.show("The language server is still loading the project …");
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        let r = ed.doc.selection().primary();
        let (start, end) = (r.from(), r.to());
        let diagnostics: Vec<wyven_lsp::Value> = ed
            .diagnostics
            .iter()
            .filter(|d| d.start <= end && start <= d.end)
            .map(|d| d.raw.clone())
            .collect();
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
            server.code_actions(path, ed.doc.text(), start, end, diagnostics);
        }
    }

    fn show_actions(&mut self, title: &str, actions: Vec<wyven_lsp::Value>) {
        if actions.is_empty() {
            self.show("No quick actions here");
            return;
        }
        let items = actions
            .into_iter()
            .map(|a| (a["title"].as_str().unwrap_or("?").to_string(), a))
            .collect();
        let fs = self.renderer.font_system();
        self.overlay = Overlay::Menu(menu::Menu::new(fs, self.ui_font, title, items));
    }

    /// Runs a picked quick action: nested ones open a sub-list, others are resolved and applied.
    fn run_action(&mut self, action: wyven_lsp::Value) {
        self.overlay = Overlay::None;
        let nested = action["data"]["NestedCodeActions"].as_array().cloned().unwrap_or_default();
        if !nested.is_empty() {
            let title = action["title"].as_str().unwrap_or("").to_string();
            self.show_actions(&title, nested);
            return;
        }
        if action["edit"].is_object() && action.get("data").is_none() {
            let what = action["title"].as_str().unwrap_or("Quick action").to_string();
            self.apply_workspace_edit(&action["edit"], &what);
            return;
        }
        if let Some(server) = server_of(&mut self.servers, &self.tabs[self.active]) {
            server.resolve_action(action);
        }
    }

    fn menu_key(&mut self, key: &Key) {
        let Overlay::Menu(menu) = &mut self.overlay else { return };
        let picked = match key {
            Key::Named(NamedKey::Escape) => {
                self.overlay = Overlay::None;
                return;
            }
            Key::Named(NamedKey::ArrowUp) => {
                menu.move_selection(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                menu.move_selection(1);
                None
            }
            Key::Named(NamedKey::Enter) => menu.selected(),
            Key::Character(c) => c
                .chars()
                .next()
                .and_then(|ch| ch.to_digit(10))
                .and_then(|n| menu.numbered(n as usize)),
            _ => None,
        };
        if let Some(action) = picked {
            self.run_action(action);
        }
    }

    /// Opens a file in a tab without switching to it; returns the tab index.
    fn open_background(&mut self, path: &Path) -> Option<usize> {
        let doc = Document::open(path).ok()?;
        let fs = self.renderer.font_system();
        let mut view = EditorView::new(doc, fs, self.font, self.family.clone());
        view.set_area(self.layout.editor);
        self.configure_view(&mut view);
        self.tabs.push(view);
        self.mru.push(self.tabs.len() - 1);
        Some(self.tabs.len() - 1)
    }

    fn tab_for(&self, path: &Path) -> Option<usize> {
        let want = path.to_string_lossy().to_lowercase();
        self.tabs
            .iter()
            .position(|t| t.doc.path().is_some_and(|p| p.to_string_lossy().to_lowercase() == want))
    }

    /// Applies a WorkspaceEdit (rename, quick action) to all files it touches. Files that were
    /// not open are opened in background tabs and stay unsaved, so everything can be reviewed
    /// and undone.
    fn apply_workspace_edit(&mut self, edit: &wyven_lsp::Value, what: &str) {
        let (per_file, mut skipped) = lsp::parse_workspace_edit(edit);
        let (mut files, mut count, mut others) = (0, 0, 0);
        for (path, edits) in per_file {
            let Some(i) = self.tab_for(&path).or_else(|| self.open_background(&path)) else {
                skipped += 1;
                continue;
            };
            let ed = &mut self.tabs[i];
            let rope = ed.doc.text();
            let changes: Vec<wyven_buffer::Change> = edits
                .iter()
                .map(|(s, e, text)| wyven_buffer::Change {
                    start: wyven_lsp::from_lsp(rope, s),
                    end: wyven_lsp::from_lsp(rope, e),
                    text: text.clone(),
                })
                .collect();
            count += changes.len();
            files += 1;
            if i != self.active {
                others += 1;
            }
            ed.doc.apply_changes(changes);
            ed.after_edit();
            let version = ed.doc.version();
            if let (Some(server), Some(p)) = (server_of(&mut self.servers, &self.tabs[i]), self.tabs[i].doc.path()) {
                server.changed(p, version);
            }
        }
        self.lsp_open_all();
        let mut msg = if files == 1 && others == 0 {
            what.to_string()
        } else {
            format!("{what}: {count} place(s) in {files} files – Ctrl+S in each tab saves")
        };
        if skipped > 0 {
            msg.push_str(&format!(" ({skipped} part(s) skipped)"));
        }
        self.show(msg);
        self.editor_mut().ensure_cursor_visible();
    }

    /// F8: jumps to the next problem after the cursor (wraps around).
    fn next_problem(&mut self) {
        let ed = self.editor_mut();
        let head = ed.doc.selection().primary().head;
        let mut starts: Vec<usize> = ed.diagnostics.iter().filter(|d| d.severity <= 2).map(|d| d.start).collect();
        starts.sort_unstable();
        starts.dedup();
        let next = starts.iter().find(|&&s| s > head).or(starts.first()).copied();
        match next {
            Some(pos) => ed.jump_to(pos),
            None => self.show("No errors or warnings in this file"),
        }
    }

    // ----- settings and themes -----

    /// Applies tab width and default indentation from the settings to a document view.
    fn configure_view(&self, view: &mut EditorView) {
        view.doc.tab_width = self.settings.editor.tab_width;
        view.minimap = editor::MinimapMode::parse(&self.settings.editor.minimap);
        view.sticky_scroll = self.settings.editor.sticky_scroll;
        view.show_inlay_hints = self.settings.editor.inlay_hints;
        view.show_code_lens = self.settings.editor.code_lens;
        view.scale = self.renderer.scale_factor();
        if !view.doc.indent_detected() {
            view.doc.indent_unit = self.settings.indent_unit();
        }
        view.invalidate();
    }

    /// Re-reads settings.toml and applies what changed. A broken file keeps the old settings.
    /// Alt → ,: the settings page as a tab (switches to it if it is open).
    fn open_settings_page(&mut self) {
        settings::ensure_file();
        if let Some(i) = self.tabs.iter().position(|t| t.settings_page) {
            self.activate(i);
        } else {
            let fs = self.renderer.font_system();
            let mut view = EditorView::virtual_doc("Settings", "", "settings", fs, self.font, self.family.clone());
            view.settings_page = true;
            view.set_area(self.layout.editor);
            self.tabs.push(view);
            self.mru.push(self.tabs.len() - 1);
            self.activate(self.tabs.len() - 1);
        }
        self.focus = Focus::Editor;
        self.window.request_redraw();
    }

    fn settings_key(&mut self, event: &KeyEvent) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        let page_lines = (self.layout.editor.h / self.ui_font.line_h).max(1.0);
        match event.key_without_modifiers() {
            Key::Character(c) if ctrl && c.as_str() == "w" => self.close_tab(self.active),
            Key::Character(c) if ctrl && c.as_str() == "q" => self.open_palette(""),
            Key::Character(c) if ctrl && c.as_str() == "v" => {
                if let Some((t, _)) = self.get_clipboard() {
                    self.settings_page.type_text(&t);
                }
            }
            Key::Named(NamedKey::Escape) => self.settings_page.escape(),
            Key::Named(NamedKey::Backspace) => self.settings_page.backspace(),
            Key::Named(NamedKey::Enter) => {
                let action = self.settings_page.commit(&self.settings);
                self.apply_settings_action(action);
            }
            Key::Named(NamedKey::ArrowUp) => self.settings_page.scroll_by(-1.0),
            Key::Named(NamedKey::ArrowDown) => self.settings_page.scroll_by(1.0),
            Key::Named(NamedKey::PageUp) => self.settings_page.scroll_by(-page_lines),
            Key::Named(NamedKey::PageDown) => self.settings_page.scroll_by(page_lines),
            _ if !ctrl && !mods.alt_key() => {
                if let Some(t) = &event.text {
                    self.settings_page.type_text(t);
                }
            }
            _ => {}
        }
        self.window.request_redraw();
    }

    fn apply_settings_action(&mut self, action: settings_page::PageAction) {
        use settings_page::PageAction as P;
        match action {
            P::None => {}
            P::Store { table, key, value } => match settings::store_value(table, key, &value) {
                Ok(()) => self.reload_settings(),
                Err(e) => self.show(format!("settings.toml: {e}")),
            },
            P::OpenFile => {
                if let Some(path) = settings::ensure_file() {
                    self.open_path(&path);
                }
            }
            P::Message(m) => self.show(m),
        }
        self.window.request_redraw();
    }

    fn reload_settings(&mut self) {
        self.settings_mtime = settings::modified();
        let new = match settings::load() {
            Ok(s) => s,
            Err(e) => {
                self.show(format!("settings.toml: {e}"));
                return;
            }
        };
        if new == self.settings {
            return;
        }
        let font_changed = new.font != self.settings.font || new.ui != self.settings.ui;
        self.settings = new;
        if let Some(theme) = Theme::by_name(&self.settings.theme) {
            self.theme = theme;
            self.terminal.dirty = true;
        } else {
            self.show(format!("Unknown theme “{}” (dark or light)", self.settings.theme));
        }
        if font_changed {
            let fs = self.renderer.font_system();
            self.family = choose_family(fs, &self.settings.font.family);
            self.font_pt = self.settings.font.size;
            self.apply_font();
        }
        let mut tabs = std::mem::take(&mut self.tabs);
        for t in &mut tabs {
            self.configure_view(t);
        }
        self.tabs = tabs;
        if let Some(ex) = &mut self.explorer {
            ex.invalidate();
        }
        let a = &self.settings.ai;
        self.ai.configure(a.enabled, ai::backend_from(a), a.delay_ms);
        self.relayout();
        if self.message.is_none() {
            self.show("Settings applied");
        }
    }

    /// Switches the theme now and writes it to settings.toml.
    fn set_theme(&mut self, name: &str) {
        let Some(theme) = Theme::by_name(name) else { return };
        self.terminal.dirty = true;
        let fs = self.renderer.font_system();
        self.find_labels.set(fs, &self.family, &theme);
        let display = theme.display_name();
        self.theme = theme;
        self.settings.theme = name.to_string();
        if let Err(e) = settings::store_theme(name) {
            self.show(format!("Theme not saved: {e}"));
        } else {
            self.settings_mtime = settings::modified();
            self.show(format!("Theme: {display}"));
        }
        for t in &mut self.tabs {
            t.invalidate();
        }
        if let Some(ex) = &mut self.explorer {
            ex.invalidate();
        }
        self.window.request_redraw();
    }

    // ----- terminal -----

    /// Alt → t: opens the terminal (starting the shell the first time), focuses it, or closes it.
    fn toggle_terminal(&mut self) {
        if self.panel_shown(Panel::Terminal) && self.focus == Focus::Terminal {
            self.dock.hide(Panel::Terminal);
            self.focus = Focus::Editor;
            self.dock_changed();
            return;
        }
        self.dock.show(Panel::Terminal);
        self.focus = Focus::Terminal;
        self.dock_changed();
        if self.terminal.term.is_none() {
            self.start_terminal();
        }
    }

    fn start_terminal(&mut self) {
        self.terminal_started = true;
        let cwd = self
            .project_root()
            .map(Path::to_path_buf)
            .or_else(|| self.editor().doc.path().and_then(Path::parent).map(Path::to_path_buf))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(std::env::temp_dir);
        let t = &self.settings.terminal;
        self.terminal.configure(&t.shell, &t.args, &cwd);
        let proxy = self.proxy.clone();
        if let Err(e) = self.terminal.start(move || {
            let _ = proxy.send_event(UserEvent::Terminal);
        }) {
            self.show(e);
        }
        self.window.request_redraw();
    }

    fn terminal_key(&mut self, event: &KeyEvent) {
        match self.terminal.key(event, self.modifiers) {
            terminal::KeyResult::Handled => {}
            terminal::KeyResult::Copy(text) => {
                self.set_clipboard((text, false));
                if let Some(t) = &self.terminal.term {
                    t.clear_selection();
                }
                self.show("Copied");
            }
            terminal::KeyResult::Paste => {
                if let Some((text, _)) = self.get_clipboard()
                    && let Some(t) = &self.terminal.term {
                        t.paste(&text);
                    }
            }
            terminal::KeyResult::Restart => self.start_terminal(),
        }
        self.terminal.dirty = true;
    }

    fn terminal_poll(&mut self) {
        let Some(t) = &mut self.terminal.term else { return };
        for event in t.poll() {
            match event {
                wyven_terminal::TermEvent::Wakeup | wyven_terminal::TermEvent::Title(_) => {}
                wyven_terminal::TermEvent::Copy(text) => self.set_clipboard((text, false)),
                wyven_terminal::TermEvent::Exited => self.show("Terminal exited – Enter restarts it"),
            }
        }
        self.terminal.dirty = true;
        self.window.request_redraw();
    }

    // ----- find and project search -----

    fn find_key(&mut self, event: &KeyEvent) {
        let mods = self.modifiers;
        let key = event.key_without_modifiers();
        let ctrl = mods.control_key() && !mods.alt_key();
        if ctrl && matches!(&key, Key::Character(c) if c.as_str() == "v") {
            if let Some((text, _)) = self.get_clipboard() {
                self.find.paste(&text);
            }
            return;
        }
        let active = self.active;
        self.find.refresh(active, &self.tabs[active].doc);
        let action = self.find.key(event, mods, &mut self.tabs[active].doc);
        // Typing in the field: jump to the first match after the cursor, like Visual Studio.
        let typed = Keymap::text(event, mods).is_some() || key == Key::Named(NamedKey::Backspace);
        self.apply_find_action(action);
        if typed && self.find.open {
            self.find.refresh(active, &self.tabs[active].doc);
            if let Some(i) = self.find.current {
                let (s, e) = self.find.matches[i];
                let ed = &mut self.tabs[active];
                ed.doc.set_selection(wyven_buffer::Selection::single(wyven_buffer::Range::new(s, e)));
                ed.ensure_cursor_visible();
            }
        }
    }

    fn apply_find_action(&mut self, action: find::FindAction) {
        match action {
            find::FindAction::None => {}
            find::FindAction::Select(s, e) => {
                let ed = self.editor_mut();
                ed.doc.set_selection(wyven_buffer::Selection::single(wyven_buffer::Range::new(s, e)));
                ed.ensure_cursor_visible();
            }
            find::FindAction::Edited => {
                let ed = self.editor_mut();
                ed.after_edit();
                ed.ensure_cursor_visible();
                self.lsp_after_edit();
                let active = self.active;
                self.find.refresh(active, &self.tabs[active].doc);
                let n = self.find.matches.len();
                if n == 0 {
                    self.show("All matches replaced");
                }
            }
            find::FindAction::Close => {
                self.find.close();
            }
        }
    }

    /// Strg+Shift+F: shows the search panel in the sidebar.
    fn open_search_panel(&mut self, replace: bool) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            self.show("No folder open: Alt → p → o opens one.");
            return;
        };
        let prefill = find::FindBar::prefill_from(&self.editor().doc);
        self.search.show(&root, replace, prefill);
        self.dock.show(Panel::Search);
        self.focus = Focus::Search;
        self.relayout();
    }

    fn search_key(&mut self, event: &KeyEvent) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        if ctrl && matches!(&event.key_without_modifiers(), Key::Character(c) if c.as_str() == "v") {
            if let Some((text, _)) = self.get_clipboard() {
                self.search.paste(&text);
            }
            return;
        }
        let action = self.search.key(event, mods);
        self.apply_panel_action(action);
    }

    fn apply_panel_action(&mut self, action: search_panel::PanelAction) {
        match action {
            search_panel::PanelAction::None => {}
            search_panel::PanelAction::Close => {
                self.close_sidebar_panel();
            }
            search_panel::PanelAction::Open { path, line, column, len } => {
                self.open_search_result(path, line, column, len);
            }
            search_panel::PanelAction::ReplaceAll => self.replace_in_project(),
        }
    }

    /// Esc in the search, git or debug panel: back to the Explorer if it shares the area,
    /// else the area closes.
    fn close_sidebar_panel(&mut self) {
        let p = self.focused_panel().unwrap_or(Panel::Search);
        let area = self.dock.area_of(p);
        if self.dock.area(area).panels.contains(&Panel::Explorer) && self.explorer.is_some() {
            self.dock.show(Panel::Explorer);
        } else {
            self.dock.hide(p);
        }
        self.focus = Focus::Editor;
        self.dock_changed();
    }

    fn open_search_result(&mut self, path: PathBuf, line: usize, column: usize, len: usize) {
        self.open_path(&path);
        let ed = self.editor_mut();
        if ed.doc.path() == Some(path.as_path()) {
            let rope = ed.doc.text();
            let line = line.min(rope.len_lines().saturating_sub(1));
            let start_byte = rope.line_to_byte(line) + column;
            let start = rope.byte_to_char(start_byte.min(rope.len_bytes()));
            let end = (start + len).min(rope.len_chars());
            ed.jump_to(start);
            ed.doc.set_selection(wyven_buffer::Selection::single(wyven_buffer::Range::new(start, end)));
        }
        // Keep the panel focused for quick browsing with arrows/Enter.
        self.focus = Focus::Search;
    }

    /// Alt+A in the search panel: replaces in all files with matches. Open files are changed in
    /// the editor (undo works), all other files are changed and saved right away.
    fn replace_in_project(&mut self) {
        let query = self.search.query.clone();
        let Ok(Some(re)) = query.compile() else { return };
        let replacement = self.search.replacement.clone();
        let paths: Vec<PathBuf> = self.search.results.iter().map(|f| f.path.clone()).collect();
        let (mut count, mut in_editor, mut saved, mut failed) = (0, 0, 0, 0);
        let changes_for = |doc: &Document| -> Vec<wyven_buffer::Change> {
            let rope = doc.text();
            let text = rope.to_string();
            wyven_search::find_all(&re, &text)
                .into_iter()
                .map(|r| wyven_buffer::Change {
                    start: rope.byte_to_char(r.start),
                    end: rope.byte_to_char(r.end),
                    text: wyven_search::replacement_for(&re, &query, &text[r.clone()], &replacement),
                })
                .collect()
        };
        for path in paths {
            if let Some(i) = self.tab_for(&path) {
                let changes = changes_for(&self.tabs[i].doc);
                count += changes.len();
                let ed = &mut self.tabs[i];
                ed.doc.apply_changes(changes);
                ed.after_edit();
                let version = ed.doc.version();
                for server in self.servers.values_mut() {
                    server.changed(&path, version);
                }
                in_editor += 1;
            } else {
                match Document::open(&path) {
                    Ok(mut doc) => {
                        let changes = changes_for(&doc);
                        count += changes.len();
                        doc.apply_changes(changes);
                        if doc.save().is_ok() { saved += 1 } else { failed += 1 }
                    }
                    Err(_) => failed += 1,
                }
            }
        }
        let mut msg = format!("{count} replacements: {saved} files saved");
        if in_editor > 0 {
            msg.push_str(&format!(", {in_editor} open file(s) changed (Ctrl+S saves)"));
        }
        if failed > 0 {
            msg.push_str(&format!(", {failed} failed"));
        }
        self.show(msg);
        self.search.schedule();
    }

    // ----- git -----

    /// Asks the background worker for the committed version of every open file.
    fn load_git_heads(&mut self) {
        if self.git.repo.is_none() {
            return;
        }
        for t in &self.tabs {
            if let Some(p) = t.doc.path() {
                self.git.load_head(p);
            }
        }
    }

    /// Files changed by another program (git, Claude Code, a formatter): load them again if
    /// they have no unsaved changes here.
    fn reload_changed_files(&mut self) {
        let mut reloaded = Vec::new();
        for t in &mut self.tabs {
            if t.reload_if_changed_on_disk()
                && let Some(p) = t.doc.path() {
                    reloaded.push(p.to_path_buf());
                }
        }
        if reloaded.is_empty() {
            return;
        }
        for server in self.servers.values_mut() {
            for p in &reloaded {
                server.close(p);
            }
        }
        self.lsp_open_all();
        if self.git.repo.is_some() {
            for p in &reloaded {
                self.git.load_head(p);
            }
            self.git.refresh_status();
        }
        if self.find.open {
            self.find.invalidate();
        }
        self.window.request_redraw();
    }

    /// Refreshes the status while the panel is visible, fetches now and then.
    fn git_tick(&mut self, now: Instant) {
        if self.git.repo.is_none() || self.git.busy.is_some() {
            return;
        }
        let visible = self.panel_shown(Panel::Git);
        let due = |last: Option<Instant>, every: Duration| last.is_none_or(|t| now.duration_since(t) >= every);
        let upstream = self.git.status.as_ref().is_some_and(|s| s.has_upstream);
        if upstream && due(self.git.last_fetch, git_panel::FETCH_INTERVAL) {
            self.git.fetch();
        } else if visible && due(self.git.last_status, git_panel::STATUS_INTERVAL) {
            self.git.refresh_status();
        }
    }

    fn git_poll(&mut self) {
        use git_panel::GitResult as R;
        for result in self.git.poll() {
            match result {
                R::Status(Ok(_)) => {}
                R::Status(Err(e)) => log::warn!("git status: {e}"),
                R::Committed(Ok(summary)) => {
                    self.git.message.clear();
                    self.show(format!("Committed: {summary}"));
                    self.git.refresh_status();
                    self.load_git_heads();
                }
                R::Synced(Ok(msg)) => {
                    self.show(msg);
                    self.git.refresh_status();
                    self.reload_changed_files();
                    self.load_git_heads();
                }
                R::Switched(Ok(branch)) => {
                    self.show(format!("Switched to “{branch}”"));
                    self.git.refresh_status();
                    self.reload_changed_files();
                    self.load_git_heads();
                }
                R::Committed(Err(e)) | R::Synced(Err(e)) | R::Switched(Err(e)) | R::Branches(Err(e)) => {
                    self.show(e);
                    self.git.refresh_status();
                }
                R::Branches(Ok(list)) => {
                    let current = self.git.status.as_ref().map(|s| s.branch.clone()).unwrap_or_default();
                    let mut items = vec![("+ new branch …".to_string(), wyven_lsp::Value::Null)];
                    items.extend(list.into_iter().map(|b| {
                        let label = if b == current { format!("{b}   (current)") } else { b.clone() };
                        (label, wyven_lsp::Value::String(b))
                    }));
                    let fs = self.renderer.font_system();
                    let title = "Switch branch (digit or Enter, Esc)";
                    self.overlay = Overlay::Branches(menu::Menu::new(fs, self.ui_font, title, items));
                }
                R::Head(path, content) => {
                    if let Some(i) = self.tab_for(&path) {
                        self.tabs[i].git_base = content;
                    }
                }
                R::Blame(Ok(b), x, y) => {
                    let text = if b.uncommitted {
                        "Not committed yet".to_string()
                    } else {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs() as i64);
                        let short: String = b.hash.chars().take(8).collect();
                        format!("{} · {}\n{}\n{short}", b.author, wyven_git::relative_time(b.time, now), b.summary)
                    };
                    let fs = self.renderer.font_system();
                    self.tooltip = Some(completion::Tooltip::new(fs, self.ui_font, &self.family, &self.theme, &text, x, y));
                }
                R::Blame(Err(e), ..) => self.show(e),
            }
        }
        self.window.request_redraw();
    }

    fn git_command(&mut self, cmd: leader::GitCmd) {
        use leader::GitCmd as G;
        if self.git.repo.is_none() {
            let msg = if self.explorer.is_none() {
                "No folder open: Alt → p → o opens one."
            } else {
                "No git repository in this folder (in the terminal: git init)."
            };
            self.show(msg);
            if !matches!(cmd, G::Panel) {
                return;
            }
        }
        let busy = self.git.busy.clone();
        if let (Some(b), G::Commit | G::Sync | G::Branch | G::NewBranch) = (&busy, cmd) {
            self.show(format!("Git is busy: {b}"));
            return;
        }
        match cmd {
            G::Panel => self.show_git_panel(),
            G::Commit => {
                if self.git.message.trim().is_empty() {
                    self.show_git_panel();
                    self.show("Type a commit message, then Ctrl+Enter");
                } else {
                    self.commit();
                }
            }
            G::Sync => self.git.sync(),
            G::Branch => self.git.branches(),
            G::NewBranch => self.start_new_branch(),
            G::Diff => match self.editor().doc.path().map(Path::to_path_buf) {
                Some(p) => self.open_diff(&p),
                None => self.show("The file has not been saved yet."),
            },
            G::Blame => {
                let ed = self.editor();
                let Some(path) = ed.doc.path().map(Path::to_path_buf) else {
                    self.show("The file has not been saved yet.");
                    return;
                };
                let head = ed.doc.selection().primary().head;
                let line = ed.doc.text().char_to_line(head);
                let (x, y) = ed.screen_pos(head);
                let y = y + ed.line_height();
                self.git.blame(&path, line, x, y);
            }
        }
    }

    fn show_git_panel(&mut self) {
        self.dock.show(Panel::Git);
        self.focus = Focus::Git;
        if self.git.repo.is_some() {
            self.git.refresh_status();
        }
        self.relayout();
    }

    /// "Immer alles committen": saves open files first, then `git add -A` + commit.
    fn commit(&mut self) {
        let mut saved = 0;
        for t in &mut self.tabs {
            if t.doc.is_dirty() && t.doc.path().is_some() && t.doc.save().is_ok() {
                t.refresh_disk_mtime();
                saved += 1;
                if let (Some(server), Some(p)) = (server_of(&mut self.servers, t), t.doc.path()) {
                    server.saved(p);
                }
            }
        }
        if saved > 0 {
            self.show(format!("{saved} file(s) saved before the commit"));
        }
        self.git.commit();
    }

    fn git_key(&mut self, event: &KeyEvent) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        if ctrl && matches!(&event.key_without_modifiers(), Key::Character(c) if c.as_str() == "v") {
            if let Some((text, _)) = self.get_clipboard() {
                self.git.paste(&text);
            }
            return;
        }
        let action = self.git.key(event, mods);
        self.apply_git_action(action);
    }

    fn apply_git_action(&mut self, action: git_panel::GitAction) {
        use git_panel::GitAction as A;
        match action {
            A::None => {}
            A::Close => self.close_sidebar_panel(),
            A::Commit => self.git_command(leader::GitCmd::Commit),
            A::OpenDiff(rel) => {
                let Some(root) = self.git.repo.as_ref().map(|r| r.root().to_path_buf()) else { return };
                let path = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                self.open_diff(&path);
                // Stay in the panel to look through the changes with arrows and Enter.
                self.focus = Focus::Git;
            }
        }
    }

    /// Shows the committed version next to the current one.
    fn open_diff(&mut self, path: &Path) {
        let Some(repo) = self.git.repo.clone() else { return };
        let Some(rel) = repo.relative(path) else {
            self.show("The file is not in the git repository.");
            return;
        };
        let old = repo.head_content(&rel).unwrap_or_default().replace("\r\n", "\n");
        let new = match self.tab_for(path) {
            Some(i) => self.tabs[i].doc.text().to_string(),
            None => std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n"),
        };
        if old == new {
            self.show(format!("{rel}: no changes since the last commit"));
            return;
        }
        let fs = self.renderer.font_system();
        let mut view = diff_view::DiffView::new(fs, self.font, self.ui_font, self.family.clone(), &self.theme, &rel, &old, &new);
        view.set_area(self.layout.editor);
        view.jump_change(true);
        self.diff = Some(view);
        self.focus = Focus::Editor;
        self.completion = None;
        self.find.close();
        self.window.request_redraw();
    }

    fn diff_key(&mut self, event: &KeyEvent) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        if self.claude_diff.is_some() && ctrl && event.key_without_modifiers() == Key::Named(NamedKey::Enter) {
            self.claude_accept();
            return;
        }
        let claude = self.claude_diff.is_some();
        let Some(d) = &mut self.diff else { return };
        match event.key_without_modifiers() {
            Key::Named(NamedKey::Escape) => self.diff = None,
            Key::Character(c) if ctrl && c.as_str() == "w" => self.diff = None,
            Key::Named(NamedKey::ArrowUp) => d.scroll_by(-1),
            Key::Named(NamedKey::ArrowDown) => d.scroll_by(1),
            Key::Named(NamedKey::PageUp) => d.scroll_by(-d.page()),
            Key::Named(NamedKey::PageDown) => d.scroll_by(d.page()),
            Key::Named(NamedKey::Home) if ctrl => d.scroll_to(0),
            Key::Named(NamedKey::End) if ctrl => d.scroll_to(usize::MAX / 2),
            Key::Named(NamedKey::F8) => {
                d.jump_change(!mods.shift_key());
            }
            Key::Character(c) if ctrl && c.as_str() == "q" => self.open_palette(""),
            _ if claude => self.show("Claude's proposal: Ctrl+Enter accepts, Esc rejects, F8 / Shift+F8 jump"),
            _ => self.show("Diff view: Esc closes, F8 / Shift+F8 jump to the next change"),
        }
    }

    fn start_new_branch(&mut self) {
        let fs = self.renderer.font_system();
        let prompt = menu::Prompt::new(fs, self.ui_font, "New branch (Enter creates and switches, Esc)", "");
        self.overlay = Overlay::NewBranch(prompt);
    }

    fn branch_menu_key(&mut self, key: &Key) {
        let Overlay::Branches(menu) = &mut self.overlay else { return };
        let picked = match key {
            Key::Named(NamedKey::Escape) => {
                self.overlay = Overlay::None;
                return;
            }
            Key::Named(NamedKey::ArrowUp) => {
                menu.move_selection(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                menu.move_selection(1);
                None
            }
            Key::Named(NamedKey::Enter) => menu.selected(),
            Key::Character(c) => c.chars().next().and_then(|ch| ch.to_digit(10)).and_then(|n| menu.numbered(n as usize)),
            _ => None,
        };
        let Some(picked) = picked else { return };
        self.overlay = Overlay::None;
        match picked.as_str() {
            Some(branch) => {
                if self.git.status.as_ref().is_some_and(|s| s.branch == branch) {
                    self.show(format!("Already on “{branch}”"));
                } else {
                    self.warn_unsaved_before_switch();
                    self.git.switch(branch.to_string(), false);
                }
            }
            None => self.start_new_branch(),
        }
    }

    fn new_branch_key(&mut self, event: &KeyEvent, key: &Key) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        let Overlay::NewBranch(prompt) = &mut self.overlay else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.overlay = Overlay::None,
            Key::Named(NamedKey::Enter) => {
                let name = prompt.text.trim().replace(' ', "-");
                self.overlay = Overlay::None;
                let valid = !name.is_empty()
                    && !name.starts_with('-')
                    && name.chars().all(|c| c.is_alphanumeric() || "-_./".contains(c));
                if !valid {
                    self.show("Invalid branch name (allowed: letters, digits, - _ . /)");
                    return;
                }
                self.git.switch(name, true);
            }
            Key::Named(NamedKey::Backspace) => prompt.backspace(ctrl),
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::End | NamedKey::Home) => prompt.keep(),
            Key::Character(c) if ctrl && c.as_str() == "v" => {
                if let Some(t) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok())
                    && let Overlay::NewBranch(prompt) = &mut self.overlay {
                        prompt.type_text(t.lines().next().unwrap_or("").trim());
                    }
            }
            _ => {
                if let Some(t) = Keymap::text(event, mods) {
                    prompt.type_text(&t);
                }
            }
        }
    }

    // ----- AI suggestions -----

    /// Typing paused: ask Claude for a suggestion at the cursor, if one makes sense here.
    fn ai_trigger(&mut self) {
        self.ai.due = None;
        let ok_place = self.focus == Focus::Editor
            && self.diff.is_none()
            && matches!(self.overlay, Overlay::None)
            && self.completion.is_none()
            && !(self.find.open && self.find.focused);
        let ed = self.editor();
        let sel = ed.doc.selection();
        if !ok_place || sel.len() != 1 || !sel.primary().is_empty() {
            return;
        }
        let rope = ed.doc.text();
        let head = sel.primary().head;
        if rope.len_chars() == 0 || rope.len_bytes() > 4_000_000 {
            return;
        }
        let line = rope.char_to_line(head);
        let line_end = rope.line_to_char(line) + wyven_buffer::line_len_chars(rope, line);
        let rest: String = rope.slice(head..line_end).to_string();
        if !ai::fits_cursor(&rest) {
            return;
        }
        // Enough context for the crate's limits, without copying a huge file.
        let start = head.saturating_sub(70_000);
        let end = (head + 8_000).min(rope.len_chars());
        let ctx = wyven_ai::Context {
            file_name: ed.file_name(),
            language: ed.language_name().to_string(),
            before: rope.slice(start..head).to_string(),
            after: rope.slice(head..end).to_string(),
        };
        let (tab, version) = (self.active, ed.doc.version());
        self.ai.start(tab, version, head, ctx);
    }

    fn ai_poll(&mut self) {
        for update in self.ai.poll(Instant::now()) {
            match update {
                ai::AiUpdate::Show { tab, version, pos, text } => {
                    let still_valid = tab == self.active
                        && self.tabs.get(tab).is_some_and(|t| {
                            t.doc.version() == version && t.doc.selection().primary().head == pos
                        });
                    if still_valid {
                        self.tabs[tab].set_ghost(Some((pos, text)));
                    }
                }
                ai::AiUpdate::Message(m) => self.show(m),
            }
        }
        self.window.request_redraw();
    }

    /// Tab, Ctrl+→ and Esc while a suggestion is shown. Returns true if the key was used.
    fn ghost_key(&mut self, key: &Key) -> bool {
        let mods = self.modifiers;
        let Some((pos, text)) = self.editor().active_ghost().map(|(p, t)| (p, t.to_string())) else { return false };
        let plain = !mods.control_key() && !mods.alt_key() && !mods.shift_key();
        let ctrl_only = mods.control_key() && !mods.alt_key() && !mods.shift_key();
        let piece = match key {
            Key::Named(NamedKey::Tab) if plain => text.clone(),
            Key::Named(NamedKey::ArrowRight) if ctrl_only => ai::next_word(&text).to_string(),
            Key::Named(NamedKey::Escape) if plain => {
                self.editor_mut().set_ghost(None);
                self.ai.cancel();
                return true;
            }
            _ => return false,
        };
        let rest = text[piece.len()..].to_string();
        let ed = self.editor_mut();
        ed.doc.insert(&piece);
        ed.after_edit();
        ed.ensure_cursor_visible();
        let new_head = ed.doc.selection().primary().head;
        ed.set_ghost((!rest.is_empty()).then_some((new_head, rest)));
        let _ = pos;
        self.lsp_after_edit();
        self.ai.cancel();
        self.window.request_redraw();
        true
    }

    fn ai_toggle(&mut self) {
        if !self.ai.usable() {
            self.ask_api_key();
            return;
        }
        let on = !self.settings.ai.enabled;
        self.settings.ai.enabled = on;
        let backend = self.ai.backend.clone();
        let delay = self.settings.ai.delay_ms;
        self.ai.configure(on, backend, delay);
        if !on {
            for t in &mut self.tabs {
                t.set_ghost(None);
            }
        }
        let label = if on { "AI suggestions on" } else { "AI suggestions off" };
        self.store_ai_setting("enabled", if on { "true" } else { "false" }, label);
    }

    /// Alt → a → m: local → Claude Haiku → Claude Sonnet → local (Claude only with a key).
    fn ai_switch_model(&mut self) {
        use wyven_ai::{Backend, Model};
        let local = Backend::Local(match &self.ai.backend {
            Backend::Local(m) => m.clone(),
            _ => self.settings.ai.local_model.clone(),
        });
        let next = match &self.ai.backend {
            Backend::Local(_) if self.ai.has_key() => Backend::Claude(Model::Haiku),
            Backend::Local(_) => {
                self.show("Only the local model is set up – Claude needs a paid API key (Alt → a → k)");
                return;
            }
            Backend::Claude(Model::Haiku) => Backend::Claude(Model::Sonnet),
            Backend::Claude(Model::Sonnet) => local,
        };
        let (provider, model) = match &next {
            Backend::Local(_) => ("local", self.settings.ai.model.clone()),
            Backend::Claude(m) => ("claude", m.name().to_string()),
        };
        self.settings.ai.provider = provider.into();
        self.settings.ai.model = model.clone();
        let (enabled, delay) = (self.settings.ai.enabled, self.settings.ai.delay_ms);
        self.ai.configure(enabled, next.clone(), delay);
        let _ = settings::store_value(Some("ai"), "model", &format!("\"{model}\""));
        let label = format!("AI model: {}", next.display());
        self.store_ai_setting("provider", &format!("\"{provider}\""), &label);
    }

    fn store_ai_setting(&mut self, key: &str, value: &str, label: &str) {
        match settings::store_value(Some("ai"), key, value) {
            Ok(()) => {
                self.settings_mtime = settings::modified();
                self.show(label.to_string());
            }
            Err(e) => self.show(format!("{label} (not saved: {e})")),
        }
        self.window.request_redraw();
    }

    fn ask_api_key(&mut self) {
        let fs = self.renderer.font_system();
        let title = "Anthropic API key (sk-ant-…), stored in the Windows Credential Manager. Enter saves, empty removes it, Esc cancels";
        let mut prompt = menu::Prompt::new(fs, self.ui_font, title, "");
        prompt.masked = true;
        self.overlay = Overlay::ApiKey(prompt);
    }

    fn api_key_key(&mut self, event: &KeyEvent, key: &Key) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        let Overlay::ApiKey(prompt) = &mut self.overlay else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.overlay = Overlay::None,
            Key::Named(NamedKey::Enter) => {
                let k = prompt.text.trim().to_string();
                self.overlay = Overlay::None;
                if !k.is_empty() && !k.starts_with("sk-ant-") {
                    self.show("That does not look like an Anthropic API key (sk-ant-…); nothing saved");
                    return;
                }
                match wyven_ai::store_api_key(&k) {
                    Ok(()) => {
                        self.ai.reload_key();
                        let msg = if k.is_empty() {
                            "API key removed"
                        } else {
                            "API key saved – Alt → a → m switches to Claude"
                        };
                        self.show(msg);
                    }
                    Err(e) => self.show(format!("API key not saved: {e}")),
                }
            }
            Key::Named(NamedKey::Backspace) => prompt.backspace(ctrl),
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::End | NamedKey::Home) => prompt.keep(),
            Key::Character(c) if ctrl && c.as_str() == "v" => {
                if let Some(t) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok())
                    && let Overlay::ApiKey(prompt) = &mut self.overlay
                {
                    prompt.type_text(t.lines().next().unwrap_or("").trim());
                }
            }
            _ => {
                if let Some(t) = Keymap::text(event, mods) {
                    prompt.type_text(&t);
                }
            }
        }
    }

    // ----- debugging -----

    /// Breakpoints of all files: open tabs plus remembered ones of closed files.
    fn all_breakpoints(&self) -> debug::Breakpoints {
        let mut all = self.breakpoint_store.clone();
        for t in &self.tabs {
            if let Some(p) = t.doc.path() {
                if t.breakpoints().is_empty() {
                    all.remove(p);
                } else {
                    all.insert(p.to_path_buf(), t.breakpoints().to_vec());
                }
            }
        }
        all
    }

    fn toggle_breakpoint(&mut self, line: usize) {
        let ed = &mut self.tabs[self.active];
        let Some(path) = ed.doc.path().map(Path::to_path_buf) else {
            self.show("Save the file first to set breakpoints.");
            return;
        };
        ed.toggle_breakpoint(line);
        let lines = ed.breakpoints().to_vec();
        self.debugger.set_breakpoints(&path, &lines);
        self.window.request_redraw();
    }

    fn show_debug_panel(&mut self, focus: bool) {
        self.dock.show(Panel::Debug);
        if focus {
            self.focus = Focus::Debug;
        }
        self.relayout();
    }

    fn debug_command(&mut self, cmd: debug::DebugCmd) {
        use debug::{DebugCmd as D, Phase};
        match cmd {
            D::StartOrContinue => match self.debugger.phase {
                Phase::Stopped => self.debugger.continue_(),
                Phase::Idle => self.start_debugging(false),
                _ => self.show("The program is already running (Shift+F5 stops it)."),
            },
            D::Stop => {
                if self.debugger.active() {
                    self.debugger.stop();
                    self.clear_debug_line();
                    self.show("Debugging stopped");
                }
            }
            D::RunWithoutDebugging => {
                if self.debugger.active() {
                    self.show("A debug session is running (Shift+F5 stops it).");
                } else {
                    self.start_debugging(true);
                }
            }
            D::StepOver => self.debugger.step("next"),
            D::StepInto => self.debugger.step("stepIn"),
            D::StepOut => self.debugger.step("stepOut"),
            D::ToggleBreakpoint => {
                let ed = self.editor();
                let line = ed.doc.text().char_to_line(ed.doc.selection().primary().head);
                self.toggle_breakpoint(line);
            }
            D::ShowPanel => self.show_debug_panel(true),
            D::AddWatch => {
                let prefill = find::FindBar::prefill_from(&self.editor().doc).unwrap_or_default();
                let fs = self.renderer.font_system();
                let prompt = menu::Prompt::new(fs, self.ui_font, "Watch expression (Enter adds, Esc)", &prefill);
                self.overlay = Overlay::Watch(prompt);
            }
            D::Attach => {
                if self.debugger.active() {
                    self.show("A debug session is already running (Shift+F5 stops it).");
                    return;
                }
                let list = debug::dotnet_processes();
                if list.is_empty() {
                    self.show("No running dotnet or Godot process found.");
                    return;
                }
                let items = list
                    .into_iter()
                    .map(|(name, pid)| (format!("{name}   (PID {pid})"), wyven_lsp::Value::from(pid)))
                    .collect();
                let fs = self.renderer.font_system();
                let title = "Attach to process (digit or Enter, Esc)";
                self.overlay = Overlay::Processes(menu::Menu::new(fs, self.ui_font, title, items));
            }
        }
    }

    /// F5 when nothing runs: picks the project, saves open files and starts the build.
    fn start_debugging(&mut self, run_only: bool) {
        let active_path = self.editor().doc.path().map(Path::to_path_buf);
        let Some(root) = self
            .project_root()
            .map(Path::to_path_buf)
            .or_else(|| active_path.as_deref().and_then(Path::parent).map(Path::to_path_buf))
        else {
            self.show("No folder open: Alt → p → o opens one.");
            return;
        };
        self.run_only = run_only;
        if let Some(p) = self.debug_project.clone()
            && p.is_file()
            && p.starts_with(&root)
        {
            self.launch_project(p);
            return;
        }
        let projects = debug::runnable_projects(&root, active_path.as_deref());
        match projects.len() {
            0 => self.show("No startable .NET project found (a .csproj with OutputType Exe)."),
            1 => self.launch_project(projects[0].clone()),
            _ => {
                let items = projects
                    .iter()
                    .map(|p| {
                        let label = p.strip_prefix(&root).unwrap_or(p).display().to_string();
                        (label, wyven_lsp::Value::String(p.display().to_string()))
                    })
                    .collect();
                let fs = self.renderer.font_system();
                let title = "Which project should F5 start? (digit or Enter, Esc)";
                self.overlay = Overlay::DebugProjects(menu::Menu::new(fs, self.ui_font, title, items));
            }
        }
    }

    fn launch_project(&mut self, project: PathBuf) {
        // Like Visual Studio: save everything before building.
        for t in &mut self.tabs {
            if t.doc.is_dirty() && t.doc.path().is_some() && t.doc.save().is_ok() {
                t.refresh_disk_mtime();
                if let (Some(server), Some(p)) = (server_of(&mut self.servers, t), t.doc.path()) {
                    server.saved(p);
                }
            }
        }
        self.debug_project = Some(project.clone());
        let name = project.file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        self.show(format!("Building {name} …"));
        if self.run_only {
            self.debugger.run_without_debugging(project);
            return;
        }
        let bps = self.all_breakpoints();
        self.debugger.start(project, bps);
        self.show_debug_panel(false);
    }

    fn debug_menu_key(&mut self, key: &Key) {
        let (Overlay::DebugProjects(menu) | Overlay::Processes(menu)) = &mut self.overlay else { return };
        let picked = match key {
            Key::Named(NamedKey::Escape) => {
                self.overlay = Overlay::None;
                return;
            }
            Key::Named(NamedKey::ArrowUp) => {
                menu.move_selection(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                menu.move_selection(1);
                None
            }
            Key::Named(NamedKey::Enter) => menu.selected(),
            Key::Character(c) => c.chars().next().and_then(|ch| ch.to_digit(10)).and_then(|n| menu.numbered(n as usize)),
            _ => None,
        };
        let Some(picked) = picked else { return };
        let overlay = std::mem::replace(&mut self.overlay, Overlay::None);
        match overlay {
            Overlay::DebugProjects(_) => {
                if let Some(p) = picked.as_str() {
                    self.launch_project(PathBuf::from(p));
                }
            }
            Overlay::Processes(_) => {
                if let Some(pid) = picked.as_u64() {
                    let bps = self.all_breakpoints();
                    match self.debugger.attach(pid as u32, bps) {
                        Ok(()) => {
                            self.show(format!("Attaching to process {pid} …"));
                            self.show_debug_panel(false);
                        }
                        Err(e) => self.show(e),
                    }
                }
            }
            _ => {}
        }
    }

    fn watch_key(&mut self, event: &KeyEvent, key: &Key) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        let Overlay::Watch(prompt) = &mut self.overlay else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.overlay = Overlay::None,
            Key::Named(NamedKey::Enter) => {
                let expr = prompt.text.trim().to_string();
                self.overlay = Overlay::None;
                if !expr.is_empty() {
                    self.debugger.add_watch(expr);
                    self.show_debug_panel(false);
                }
            }
            Key::Named(NamedKey::Backspace) => prompt.backspace(ctrl),
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::End | NamedKey::Home) => prompt.keep(),
            Key::Character(c) if ctrl && c.as_str() == "v" => {
                if let Some(t) = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok())
                    && let Overlay::Watch(prompt) = &mut self.overlay
                {
                    prompt.type_text(t.lines().next().unwrap_or("").trim());
                }
            }
            _ => {
                if let Some(t) = Keymap::text(event, mods) {
                    prompt.type_text(&t);
                }
            }
        }
    }

    fn apply_debug_panel_action(&mut self, action: debug::PanelAction) {
        match action {
            debug::PanelAction::None => {}
            debug::PanelAction::Close => self.close_sidebar_panel(),
            debug::PanelAction::Jump(path, line) => {
                self.open_path(&path);
                if self.editor().doc.path() == Some(path.as_path()) {
                    self.editor_mut().jump_to_line(line + 1, None);
                }
                self.focus = Focus::Debug;
            }
            debug::PanelAction::RemoveBreakpoint(path, line) => {
                if let Some(i) = self.tab_for(&path) {
                    let t = &mut self.tabs[i];
                    t.toggle_breakpoint(line);
                    let lines = t.breakpoints().to_vec();
                    self.debugger.set_breakpoints(&path, &lines);
                } else if let Some(lines) = self.breakpoint_store.get_mut(&path) {
                    lines.retain(|&l| l != line);
                    let lines = lines.clone();
                    if lines.is_empty() {
                        self.breakpoint_store.remove(&path);
                    }
                    self.debugger.set_breakpoints(&path, &lines);
                }
            }
        }
    }

    fn clear_debug_line(&mut self) {
        for t in &mut self.tabs {
            t.set_debug_line(None);
        }
    }

    fn debug_poll(&mut self) {
        for action in self.debugger.poll() {
            match action {
                debug::DebugAction::ShowLocation(path, line) => {
                    self.clear_debug_line();
                    if path.is_file() {
                        self.open_path(&path);
                        if self.editor().doc.path() == Some(path.as_path()) {
                            let ed = self.editor_mut();
                            ed.jump_to_line(line + 1, None);
                            ed.set_debug_line(Some(line));
                        }
                    }
                    // Like Visual Studio: come to the front when the program pauses.
                    self.window.focus_window();
                }
                debug::DebugAction::ClearLocation => self.clear_debug_line(),
                debug::DebugAction::Message(m) => self.show(m),
                debug::DebugAction::Hover { text, x, y } => {
                    let fs = self.renderer.font_system();
                    self.tooltip = Some(completion::Tooltip::new(fs, self.ui_font, &self.family, &self.theme, &text, x, y));
                }
                debug::DebugAction::Ended => {}
            }
        }
        self.window.request_redraw();
    }

    fn warn_unsaved_before_switch(&mut self) {
        let dirty = self.tabs.iter().filter(|t| t.doc.is_dirty()).count();
        if dirty > 0 {
            self.show(format!("{dirty} file(s) unsaved: they keep their editor content and are not reloaded"));
        }
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
            None => self.show(format!("Alt → {group}{c} is not assigned (Alt → h shows all shortcuts)")),
        }
    }

    fn leader_action(&mut self, action: leader::Action) {
        use leader::Action::*;
        match action {
            Group(_) => {}
            ToggleExplorer => {
                if self.explorer.is_none() {
                    self.show("No folder open: Alt → p → o opens one.");
                    return;
                }
                if self.panel_shown(Panel::Explorer) {
                    self.dock.hide(Panel::Explorer);
                    self.focus = Focus::Editor;
                } else {
                    self.dock.show(Panel::Explorer);
                    self.focus = Focus::Explorer;
                }
                self.dock_changed();
            }
            FocusExplorer => {
                if self.explorer.is_some() {
                    self.dock.show(Panel::Explorer);
                    self.focus = Focus::Explorer;
                    self.dock_changed();
                }
            }
            ResetLayout => self.reset_layout(),
            OpenFolder => self.pick_folder(),
            RecentProjects => {
                let list = config::recent_projects();
                if list.is_empty() {
                    self.show("No recent projects yet.");
                } else {
                    self.overlay = Overlay::Recent(list);
                }
            }
            Palette(prefix) => self.open_palette(prefix),
            Run(cmd) => self.run(cmd),
            OpenFile => self.pick_file(),
            NewFile => self.new_file(),
            SaveAs => self.save_as(),
            CloseAll => self.close_all(),
            Later(msg) => self.show(msg),
            OpenSettings => self.open_settings_page(),
            OpenSettingsFile => {
                if let Some(path) = settings::ensure_file() {
                    self.open_path(&path);
                }
            }
            CheatSheet => {
                let fs = self.renderer.font_system();
                let mut view = EditorView::virtual_doc(
                    "Keyboard Shortcuts",
                    &help::cheat_sheet(),
                    "help.md",
                    fs,
                    self.font,
                    self.family.clone(),
                );
                view.set_area(self.layout.editor);
                self.configure_view(&mut view);
                self.tabs.push(view);
                self.mru.push(self.tabs.len() - 1);
                self.activate(self.tabs.len() - 1);
                self.focus = Focus::Editor;
            }
            SetTheme(name) => self.set_theme(name),
            ToggleTerminal => self.toggle_terminal(),
            ToggleTheme => {
                let next = if self.theme.name == "dark" { "light" } else { "dark" };
                self.set_theme(next);
            }
            Git(cmd) => self.git_command(cmd),
            CycleMinimap => self.cycle_minimap(),
            MarkdownPreview => self.toggle_preview(),
            ClaudeCode => self.launch_claude_code(),
            AiToggle => self.ai_toggle(),
            AiModel => self.ai_switch_model(),
            AiKey => self.ask_api_key(),
            ToggleStickyScroll => self.toggle_sticky_scroll(),
            ToggleInlayHints => self.toggle_extra("inlay_hints"),
            ToggleCodeLens => self.toggle_extra("code_lens"),
        }
    }

    /// Alt → v → s: sticky scroll on/off, saved in settings.toml.
    fn toggle_sticky_scroll(&mut self) {
        let on = !self.settings.editor.sticky_scroll;
        self.settings.editor.sticky_scroll = on;
        for t in &mut self.tabs {
            t.sticky_scroll = on;
            t.invalidate();
        }
        let label = if on { "Sticky scroll: on" } else { "Sticky scroll: off" };
        match settings::store_value(Some("editor"), "sticky_scroll", if on { "true" } else { "false" }) {
            Ok(()) => {
                self.settings_mtime = settings::modified();
                self.show(label);
            }
            Err(e) => self.show(format!("{label} (not saved: {e})")),
        }
    }

    /// Alt → v → i / l: inlay hints or CodeLens on/off, saved in settings.toml.
    fn toggle_extra(&mut self, key: &str) {
        let e = &mut self.settings.editor;
        let (on, label) = if key == "inlay_hints" {
            e.inlay_hints = !e.inlay_hints;
            (e.inlay_hints, "Inlay hints")
        } else {
            e.code_lens = !e.code_lens;
            (e.code_lens, "CodeLens")
        };
        let mut tabs = std::mem::take(&mut self.tabs);
        for t in &mut tabs {
            self.configure_view(t);
        }
        self.tabs = tabs;
        for server in self.servers.values_mut() {
            server.want_inlay_hints = self.settings.editor.inlay_hints;
            server.want_code_lens = self.settings.editor.code_lens;
            if on {
                server.refresh_extras();
            }
        }
        let label = format!("{label}: {}", if on { "on" } else { "off" });
        match settings::store_value(Some("editor"), key, if on { "true" } else { "false" }) {
            Ok(()) => {
                self.settings_mtime = settings::modified();
                self.show(label);
            }
            Err(e) => self.show(format!("{label} (not saved: {e})")),
        }
        self.window.request_redraw();
    }

    /// Shift+F12, Alt → j → r or a CodeLens click: asks the C# server for all references.
    fn request_references(&mut self, pos: usize) {
        if self.tabs[self.active].lsp_kind().is_none() {
            self.show("Find references needs a language server (C#, TypeScript, HTML, CSS, JSON).");
            return;
        }
        if !self.lsp_ready() {
            self.show("The language server is still loading the project …");
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
            server.references(path, ed.doc.text(), pos);
        }
    }

    /// Shows the references as a list: "File.cs:12   the code on that line".
    fn show_references(&mut self, refs: Vec<(PathBuf, wyven_lsp::Value)>) {
        if refs.is_empty() {
            self.show("No references found");
            return;
        }
        let mut items = Vec::new();
        for (path, start) in &refs {
            let line = start["line"].as_u64().unwrap_or(0) as usize;
            let code = match self.tab_for(path) {
                Some(i) => self.tabs[i].doc.text().get_line(line).map(|l| l.to_string()).unwrap_or_default(),
                None => std::fs::read_to_string(path)
                    .ok()
                    .and_then(|t| t.lines().nth(line).map(str::to_string))
                    .unwrap_or_default(),
            };
            let label = format!("{}:{}   {}", file_name(path), line + 1, code.trim());
            let value = wyven_lsp::json!({ "path": path.display().to_string(), "start": start });
            items.push((label, value));
        }
        let title = format!("{} reference{} (digit or Enter, Esc)", refs.len(), if refs.len() == 1 { "" } else { "s" });
        let fs = self.renderer.font_system();
        self.overlay = Overlay::References(menu::Menu::new(fs, self.ui_font, &title, items));
    }

    fn references_key(&mut self, key: &Key) {
        let Overlay::References(menu) = &mut self.overlay else { return };
        let picked = match key {
            Key::Named(NamedKey::Escape) => {
                self.overlay = Overlay::None;
                return;
            }
            Key::Named(NamedKey::ArrowUp) => {
                menu.move_selection(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                menu.move_selection(1);
                None
            }
            Key::Named(NamedKey::Enter) => menu.selected(),
            Key::Character(c) => c.chars().next().and_then(|ch| ch.to_digit(10)).and_then(|n| menu.numbered(n as usize)),
            _ => None,
        };
        let Some(v) = picked else { return };
        self.overlay = Overlay::None;
        let Some(path) = v["path"].as_str().map(PathBuf::from) else { return };
        self.open_path(&path);
        let ed = self.editor_mut();
        if ed.doc.path() == Some(path.as_path()) {
            let pos = wyven_lsp::from_lsp(ed.doc.text(), &v["start"]);
            ed.jump_to(pos);
        }
    }

    /// Alt → v → m: blocks → bar → off, saved in settings.toml.
    fn cycle_minimap(&mut self) {
        let next = editor::MinimapMode::parse(&self.settings.editor.minimap).next();
        self.settings.editor.minimap = next.name().to_string();
        for t in &mut self.tabs {
            t.minimap = next;
            t.invalidate();
        }
        let label = match next {
            editor::MinimapMode::Blocks => "Minimap: code blocks",
            editor::MinimapMode::Bar => "Minimap: marker bar",
            editor::MinimapMode::Off => "Minimap: off",
        };
        match settings::store_value(Some("editor"), "minimap", &format!("\"{}\"", next.name())) {
            Ok(()) => {
                self.settings_mtime = settings::modified();
                self.show(label);
            }
            Err(e) => self.show(format!("{label} (not saved: {e})")),
        }
    }

    fn run(&mut self, cmd: Command) {
        let mut follow_cursor = true;
        let typed = if let Command::Insert(t) = &cmd { Some(t.clone()) } else { None };
        let keeps_completion =
            matches!(cmd, Command::Insert(_) | Command::Backspace | Command::TriggerCompletion);
        let mut trigger = None;
        let bps_before = self.editor().breakpoints().to_vec();
        // Typing asks the AI again once typing pauses; anything else drops its suggestion.
        let typing = matches!(
            cmd,
            Command::Insert(_) | Command::Newline | Command::Backspace | Command::Delete | Command::DeleteWord { .. }
        );
        let ed = &mut self.tabs[self.active];
        match cmd {
            Command::Move(m, extend) => {
                let m = match m {
                    Movement::PageUp(_) => Movement::PageUp(ed.page_lines()),
                    Movement::PageDown(_) => Movement::PageDown(ed.page_lines()),
                    m => m,
                };
                let old = ed.doc.selection().primary().head;
                ed.doc.move_cursors(m, extend);
                if !extend && ed.doc.selection().len() == 1 {
                    ed.skip_folds(old);
                }
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
                self.find.close();
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
                self.font_pt = self.settings.font.size;
                self.apply_font();
                follow_cursor = false;
            }
            Command::Debug(c) => {
                follow_cursor = false;
                self.debug_command(c);
            }
            Command::Fold(f) => {
                if !ed.fold(f) {
                    self.show("Nothing to fold here");
                }
            }
            Command::TriggerCompletion => trigger = Some(None),
            Command::GotoDefinition => {
                follow_cursor = false;
                let pos = self.editor().doc.selection().primary().head;
                self.request_definition(pos);
            }
            Command::FindReferences => {
                follow_cursor = false;
                let pos = self.editor().doc.selection().primary().head;
                self.request_references(pos);
            }
            Command::Format => {
                follow_cursor = false;
                self.request_format();
            }
            Command::Find { replace } => {
                follow_cursor = false;
                let prefill = find::FindBar::prefill_from(&self.editor().doc);
                self.find.show(replace, prefill);
                self.focus = Focus::Editor;
            }
            Command::FindNext { back } => {
                follow_cursor = false;
                if self.find.query.text.is_empty() {
                    let prefill = find::FindBar::prefill_from(&self.editor().doc);
                    self.find.show(false, prefill);
                }
                self.find.open = true;
                let active = self.active;
                self.find.refresh(active, &self.tabs[active].doc);
                let action = self.find.step(&self.tabs[active].doc, !back);
                self.apply_find_action(action);
            }
            Command::ProjectSearch { replace } => {
                follow_cursor = false;
                self.open_search_panel(replace);
            }
            Command::Rename => {
                follow_cursor = false;
                self.start_rename();
            }
            Command::QuickActions => {
                follow_cursor = false;
                self.request_quick_actions();
            }
            Command::NextProblem => {
                follow_cursor = false;
                self.next_problem();
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
        // Edits move breakpoints; keep a running debug session in sync.
        if self.debugger.active() && self.editor().breakpoints() != bps_before.as_slice()
            && let Some(path) = self.editor().doc.path().map(Path::to_path_buf)
        {
            let lines = self.editor().breakpoints().to_vec();
            self.debugger.set_breakpoints(&path, &lines);
        }
        self.lsp_after_edit();
        if !keeps_completion {
            self.completion = None;
        }
        self.update_completion(typed.as_deref(), trigger);
        self.editor_mut().set_ghost(None);
        if typing && self.focus == Focus::Editor {
            self.ai.schedule(Instant::now());
        } else {
            self.ai.cancel();
        }
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
            self.mouse.minimap_drag = None;
            if self.resizing.take().is_some() {
                self.dock.save();
            }
            self.panel_drop();
            if self.terminal.selecting {
                self.terminal.selecting = false;
                // A click without dragging selects nothing.
                if let Some(t) = &self.terminal.term
                    && t.selection_text().is_none_or(|s| s.trim().is_empty()) {
                        t.clear_selection();
                        self.terminal.dirty = true;
                        self.window.request_redraw();
                    }
            }
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

        let st = self.layout.status;
        if button == MouseButton::Left
            && y >= st.y
            && x >= self.status_ai_span.0
            && x < self.status_ai_span.1
        {
            self.ai_toggle();
            return;
        }
        if button == MouseButton::Left {
            if let Some(area) = self.splitter_at(x, y) {
                self.resizing = Some(area);
                return;
            }
            if let Some(tab) = self.dock_bar.hit(x, y) {
                self.panel_drag = Some(PanelDrag { panel: tab.panel, from: (x, y), moved: false, target: None });
                self.activate_panel(tab.panel);
                return;
            }
        }
        let t = self.layout.tabs;
        if y >= t.y && y < t.y + t.h && x >= t.x && x < t.x + t.w {
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

        if self.panel_shown(Panel::Terminal) && self.terminal.contains(x, y) {
            self.focus = Focus::Terminal;
            if button == MouseButton::Left {
                let (row, col) = self.terminal.cell_at(x, y);
                if let Some(t) = &self.terminal.term {
                    t.select_start(row, col);
                }
                self.terminal.selecting = true;
                self.terminal.dirty = true;
            }
            return;
        }
        if self.find.contains(x, y) {
            self.find.click(x, y);
            self.focus = Focus::Editor;
            return;
        }
        if self.panel_shown(Panel::Search) && self.search.contains(x, y) {
            self.focus = Focus::Search;
            if button == MouseButton::Left {
                let action = self.search.click(y);
                self.apply_panel_action(action);
            }
            return;
        }
        if self.panel_shown(Panel::Git) && self.git.contains(x, y) {
            self.focus = Focus::Git;
            if button == MouseButton::Left {
                let action = self.git.click(y);
                self.apply_git_action(action);
            }
            return;
        }
        if self.panel_shown(Panel::Debug) && self.debugger.contains(x, y) {
            self.focus = Focus::Debug;
            if button == MouseButton::Left {
                let action = self.debugger.click(y);
                self.apply_debug_panel_action(action);
            }
            return;
        }
        if let Some(d) = &self.diff
            && d.contains(x, y) {
                self.focus = Focus::Editor;
                return;
            }
        if self.editor().settings_page && self.settings_page.contains(x, y) {
            self.focus = Focus::Editor;
            if button == MouseButton::Left {
                let action = self.settings_page.click(x, y, &self.settings);
                self.apply_settings_action(action);
            }
            return;
        }
        let explorer_shown = self.panel_shown(Panel::Explorer);
        if let Some(ex) = &mut self.explorer
            && explorer_shown && ex.contains(x, y) {
                self.focus = Focus::Explorer;
                if button == MouseButton::Left
                    && let Some(i) = ex.row_at(y)
                        && let Some(path) = ex.activate(i) {
                            self.open_path(&path);
                        }
                return;
            }

        if button == MouseButton::Left
            && let Some(start) = self.editor_mut().minimap_press(x, y)
        {
            self.mouse.minimap_drag = Some((y, start));
            self.focus = Focus::Editor;
            return;
        }
        if button != MouseButton::Left || !self.editor().contains(x, y) {
            return;
        }
        if self.diff.is_none()
            && let Some(pos) = self.editor().lens_at(x, y)
        {
            self.focus = Focus::Editor;
            self.request_references(pos);
            return;
        }
        if self.diff.is_none() && (self.editor_mut().sticky_click(x, y) || self.editor_mut().fold_click(x, y)) {
            self.focus = Focus::Editor;
            self.find.focused = false;
            return;
        }
        if self.diff.is_none()
            && let Some(line) = self.editor().line_number_at(x, y)
        {
            self.toggle_breakpoint(line);
            self.focus = Focus::Editor;
            return;
        }
        self.focus = Focus::Editor;
        self.find.focused = false;
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
        if self.panel_shown(Panel::Terminal) && self.terminal.contains(x, y) {
            if let Some(t) = &self.terminal.term {
                t.scroll(dy.round() as i32);
            }
            self.terminal.dirty = true;
            self.window.request_redraw();
            return;
        }
        if self.panel_shown(Panel::Search) && self.search.contains(x, y) {
            self.search.scroll_by(-dy.round() as isize);
            self.window.request_redraw();
            return;
        }
        if self.panel_shown(Panel::Git) && self.git.contains(x, y) {
            self.git.scroll_by(-dy.round() as isize);
            self.window.request_redraw();
            return;
        }
        if self.panel_shown(Panel::Debug) && self.debugger.contains(x, y) {
            self.debugger.scroll_by(-dy.round() as isize);
            self.window.request_redraw();
            return;
        }
        if self.diff.is_none() && self.editor().settings_page && self.settings_page.contains(x, y) {
            self.settings_page.scroll_by(-dy.round());
            self.window.request_redraw();
            return;
        }
        if let Some(d) = &mut self.diff
            && d.contains(x, y) {
                d.scroll_by(-dy.round() as isize);
                self.window.request_redraw();
                return;
            }
        let explorer_shown = self.panel_shown(Panel::Explorer);
        if let Some(ex) = &mut self.explorer
            && explorer_shown && ex.contains(x, y) {
                ex.scroll_by(-dy.round() as isize);
                self.window.request_redraw();
                return;
            }
        // The preview follows the editor: the wheel over it scrolls the text.
        if self.preview.as_ref().is_some_and(|p| p.contains(x, y)) {
            self.editor_mut().scroll_by(-dy.round() as isize);
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
        let title = format!("{dirty}{}{project} – Wyven", ed.file_name());
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
        let mut left = format!("Ln {}, Col {}", line + 1, col + 1);
        if cursors > 1 {
            left.push_str(&format!("   {cursors} cursors"));
        }
        if self.focus == Focus::Explorer {
            left.push_str("   ·   Explorer (Esc = back to the editor)");
        }
        if self.focus == Focus::Terminal && self.panel_shown(Panel::Terminal) {
            left = "Terminal".to_string();
        }
        if let Some(d) = &self.diff {
            left = format!("Diff: {}", d.title);
        }
        if self.debugger.active() {
            left = format!("Debug: {}   ·   {left}", self.debugger.status_text());
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
        let language = match ed.lsp_kind().and_then(|k| self.servers.get(&k)) {
            Some(server) => server.status_text(),
            None => ed.language_name().to_string(),
        };
        let ending = match ed.doc.line_ending() {
            wyven_buffer::LineEnding::Lf => "LF",
            wyven_buffer::LineEnding::CrLf => "CRLF",
        };
        let ram = metrics::ram_mb().map_or("–".into(), |mb| format!("{mb:.0} MB"));
        let right = format!(
            "{problems}{language}  ·  {ending}  ·  RAM {ram}  ·  key→frame {} (avg {})",
            metrics::ms(self.latency.last()),
            metrics::ms(self.latency.average()),
        );
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let fs = self.renderer.font_system();
        self.status_left.set_text(&left, &attrs, Shaping::Advanced, None);
        self.status_left.shape_until_scroll(fs, false);
        self.status_right.set_text(&right, &attrs, Shaping::Advanced, None);
        self.status_right.shape_until_scroll(fs, false);
        let ai_label = self.ai.label();
        self.status_ai.set_text(&ai_label, &attrs, Shaping::Advanced, None);
        self.status_ai.shape_until_scroll(fs, false);
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
                "Recent projects (pick a digit, Esc = cancel)".into(),
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
        if !self.terminal_started && self.panel_shown(Panel::Terminal) {
            self.start_terminal();
        }
        self.claude_check_diff();
        self.claude_send_selection();
        if self.preview_visible() != self.layout.preview.is_some() {
            self.relayout();
        }
        self.prepare_preview();
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
        match &mut self.overlay {
            Overlay::Menu(m) => m.prepare(self.renderer.font_system(), &self.theme, &self.family),
            Overlay::Rename { prompt, .. } | Overlay::NewBranch(prompt) => {
                prompt.prepare(self.renderer.font_system(), &self.theme, &self.family)
            }
            Overlay::Branches(m) | Overlay::DebugProjects(m) | Overlay::Processes(m) | Overlay::References(m) => {
                m.prepare(self.renderer.font_system(), &self.theme, &self.family)
            }
            Overlay::Watch(prompt) | Overlay::ApiKey(prompt) => {
                prompt.prepare(self.renderer.font_system(), &self.theme, &self.family)
            }
            _ => {}
        }
        if self.panel_shown(Panel::Terminal) {
            let focused = self.focus == Focus::Terminal;
            let fs = self.renderer.font_system();
            self.terminal.prepare(fs, &self.theme, focused);
        }
        let active_path = self.editor().doc.path().map(Path::to_path_buf);
        {
            let theme = &self.theme;
            let fs = self.renderer.font_system();
            match &mut self.diff {
                Some(d) => d.prepare(fs, theme),
                None if self.tabs[self.active].settings_page => self.settings_page.prepare(fs, theme, &self.settings),
                None => self.tabs[self.active].prepare(fs, theme),
            }
            if let Some(ex) = &mut self.explorer {
                ex.prepare(self.renderer.font_system(), theme, active_path.as_deref());
            }
        }
        if self.panel_shown(Panel::Search) {
            let focused = self.focus == Focus::Search;
            self.search.prepare(self.renderer.font_system(), &self.theme, focused);
        }
        if self.panel_shown(Panel::Git) {
            let focused = self.focus == Focus::Git;
            self.git.prepare(self.renderer.font_system(), &self.theme, focused);
        }
        if self.panel_shown(Panel::Debug) {
            let focused = self.focus == Focus::Debug;
            let bps = self.all_breakpoints();
            self.debugger.prepare(self.renderer.font_system(), &self.theme, focused, &bps);
        }
        let active = self.active;
        if self.find.open {
            self.find.refresh(active, &self.tabs[active].doc);
            self.tabs[active].find_matches = self.find.matches.clone();
            self.tabs[active].find_current = self.find.current;
            self.find.prepare(self.renderer.font_system(), &self.theme, &self.family);
            let (editor_area, scale) = (self.layout.editor, self.renderer.scale_factor());
            self.find.place(self.ui_font, editor_area, scale);
        } else {
            self.tabs[active].find_matches.clear();
            self.tabs[active].find_current = None;
        }

        // Which panels show (before the panels hand out their text for this frame).
        let shown = Panel::ALL.map(|p| self.panel_shown(p));
        let focused_panel = self.focused_panel();
        let strips: Vec<(Area, Rect, Option<Panel>)> = Area::ALL
            .into_iter()
            .filter_map(|a| self.layout_area(a).map(|r| (a, r, self.dock.area(a).active)))
            .collect();
        let theme = &self.theme;
        let scale = self.renderer.scale_factor();
        let l = self.layout;
        let mut under = Vec::with_capacity(64);
        let mut over = Vec::with_capacity(16);
        let mut overlay = Vec::new();
        let mut text: Vec<TextBlock<'_>> = Vec::with_capacity(8);
        let mut overlay_text: Vec<TextBlock<'_>> = Vec::new();

        if shown[1] {
            self.search.draw(theme, self.focus == Focus::Search, &mut under, &mut text);
        }
        if shown[2] {
            self.git.draw(theme, self.focus == Focus::Git, &mut under, &mut text);
        }
        if shown[3] {
            self.debugger.draw(theme, self.focus == Focus::Debug, &mut under, &mut text);
        }
        if let (Some(ex), true) = (&self.explorer, shown[0]) {
            ex.draw(theme, self.focus == Focus::Explorer, &mut under, &mut text);
        }
        if let Some(r) = l.left {
            under.push(Quad { x: r.x + r.w - 1.0, y: r.y, w: 1.0, h: r.h, color: theme.divider });
        }
        if let Some(r) = l.right {
            under.push(Quad { x: r.x, y: r.y, w: 1.0, h: r.h, color: theme.divider });
        }
        self.dock_bar.draw(theme, scale, &strips, focused_panel, &mut under, &mut text);
        if let Some((area, _)) = self.panel_drag.as_ref().filter(|d| d.moved).and_then(|d| d.target) {
            let body = Rect { x: 0.0, y: 0.0, w: l.status.w, h: l.status.y };
            let open = match area {
                Area::Left => l.left,
                Area::Right => l.right,
                Area::Bottom => l.bottom,
            };
            let z = dock::drop_zone(area, body, open);
            overlay.push(Quad { x: z.x, y: z.y, w: z.w, h: z.h, color: theme.accent.with_alpha(0x38) });
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

        match &self.diff {
            Some(d) => d.draw(theme, scale, &mut under, &mut over, &mut text),
            None if self.tabs[self.active].settings_page => self.settings_page.draw(theme, &mut under, &mut text),
            None => self.tabs[self.active].draw(theme, scale, &mut under, &mut over, &mut text),
        }
        if let Some(p) = &self.preview {
            p.draw(theme, scale, &mut under, &mut text);
        }
        if shown[4] {
            let focused = self.focus == Focus::Terminal;
            self.terminal.draw(theme, focused, scale, &mut under, &mut over, &mut text);
        }

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
        // The AI switch, left of the right-hand status text (click toggles).
        let ai_w = self.status_ai.layout_runs().next().map_or(0.0, |r| r.line_w);
        let ai_x = s.w - right_w - 3.0 * pad - ai_w;
        self.status_ai_span = (ai_x - pad * 0.5, ai_x + ai_w + pad * 0.5);
        let ai_color = if self.ai.enabled && self.ai.has_key() { theme.status_accent } else { theme.status_text };
        text.push(TextBlock { buffer: &self.status_ai, x: ai_x, y: text_y, color: ai_color, clip: None });

        if popup_visible {
            self.popup.draw(theme, self.ui_font, s.w, s.y, &mut overlay, &mut overlay_text);
        }
        let window = (s.w, s.y + s.h);
        if self.find.open {
            self.find.draw(theme, self.ui_font, scale, &mut overlay, &mut overlay_text);
            self.find_labels.draw(&self.find, self.ui_font, theme, &mut overlay_text);
        }
        if let Some(c) = &self.completion {
            let ed = &self.tabs[self.active];
            let (x, y) = ed.screen_pos(c.anchor);
            let lh = ed.line_height();
            c.draw(theme, self.ui_font, x - self.ui_font.cell_w * 0.75, y + lh, y, window, scale, &mut overlay, &mut overlay_text);
        }
        if let Some(t) = &self.tooltip {
            t.draw(theme, self.ui_font, window, scale, &mut overlay, &mut overlay_text);
        }
        {
            let ed = &self.tabs[self.active];
            let (cx, cy) = ed.screen_pos(ed.doc.selection().primary().head);
            let below = cy + ed.line_height();
            match &self.overlay {
                Overlay::Menu(m) => m.draw(theme, self.ui_font, cx, below, window, scale, &mut overlay, &mut overlay_text),
                Overlay::Rename { prompt, .. } => {
                    prompt.draw(theme, self.ui_font, cx, below, window, scale, &mut overlay, &mut overlay_text)
                }
                // Git popups belong to no cursor: show them near the top of the editor.
                Overlay::Branches(m) | Overlay::DebugProjects(m) | Overlay::Processes(m) | Overlay::References(m) => {
                    let (x, y) = (l.editor.x + self.ui_font.cell_w * 4.0, l.editor.y + self.ui_font.line_h);
                    m.draw(theme, self.ui_font, x, y, window, scale, &mut overlay, &mut overlay_text)
                }
                Overlay::NewBranch(prompt) | Overlay::Watch(prompt) | Overlay::ApiKey(prompt) => {
                    let (x, y) = (l.editor.x + self.ui_font.cell_w * 4.0, l.editor.y + self.ui_font.line_h);
                    prompt.draw(theme, self.ui_font, x, y, window, scale, &mut overlay, &mut overlay_text)
                }
                _ => {}
            }
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
