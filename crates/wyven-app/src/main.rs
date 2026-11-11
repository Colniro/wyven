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
mod chrome;
mod compass;
mod config;
mod ctxmenu;
mod custom_keys;
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
mod vscode_theme;
mod workspace;
mod plugins;
mod update;

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
    Renderer, Rgba, RoundRect, Scene, Shaping, TextBlock, Wrap,
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
    Update,
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
    // wyven [folder, file or .code-workspace] [--terminal] [--git] [--diff] [--goto=LINE] [--preview] [--settings] [--zen] [--split]
    let mut path = None;
    let (mut open_terminal, mut open_git, mut open_diff) = (false, false, false);
    let mut goto_line: Option<usize> = None;
    let mut open_preview = false;
    let mut open_settings = false;
    let mut open_zen = false;
    let mut open_split = false;
    let mut open_popup: Option<String> = None;
    let mut demo: Option<String> = None;
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
        } else if arg == "--zen" {
            open_zen = true;
        } else if arg == "--split" {
            open_split = true;
        } else if arg == "--compass" || arg == "--menu" {
            // Screenshots: a running drag with the compass, or a right-click menu.
            demo = arg.to_str().map(str::to_string);
        } else if let Some(g) = arg.to_str().and_then(|a| a.strip_prefix("--popup=")) {
            // Shows a leader menu right away (for screenshots of the key popup).
            open_popup = Some(g.to_string());
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
    let mut app = App { path, open_terminal, open_git, open_diff, goto_line, open_preview, open_settings, open_zen, open_split, open_popup, demo, state: None, proxy };
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
    open_zen: bool,
    open_split: bool,
    open_popup: Option<String>,
    demo: Option<String>,
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

/// File operations in the explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerOp {
    Rename,
    Delete,
    NewFile,
    NewFolder,
}

/// The theme picker: every theme (label, settings name), the filter typed so far, and the
/// theme to go back to on Esc.
struct ThemePicker {
    all: Vec<(String, String)>,
    filter: String,
    before: String,
}

/// The second editor half. The half with the focus is always `State::tabs`; this holds the
/// other one (switching focus swaps them), so every command simply works on the focused half.
struct Split {
    /// Side by side (true) or stacked.
    vertical: bool,
    tabs: Vec<EditorView>,
    active: usize,
    mru: Vec<usize>,
    /// The focused half is the left/top one.
    focused_first: bool,
    tab_bar: Buffer,
    tab_spans: Vec<(f32, f32)>,
}

/// Colored tab labels and the x range of each tab.
type TabBar = (Vec<(String, Rgba)>, Vec<(f32, f32)>);

/// Cells at the end of a tab for its close button.
const TAB_CLOSE_CELLS: usize = 4;

/// The close button of a tab spanning `x0..x1` (cell width `cw`): its x range.
fn tab_close_span((_, x1): (f32, f32), cw: f32) -> (f32, f32) {
    (x1 - cw * (TAB_CLOSE_CELLS as f32 - 0.6), x1 - cw * 0.9)
}

/// Labels and screen spans of a tab bar.
fn tab_bar_spans(tabs: &[EditorView], active: usize, x0: f32, cw: f32, theme: &Theme) -> TabBar {
    let mut x = x0;
    let (mut labels, mut spans) = (Vec::new(), Vec::new());
    for (i, t) in tabs.iter().enumerate() {
        // "  C# Program.cs    ": badge, name, and room for the close button (drawn on top).
        let name = t.file_name();
        let (badge, badge_color) = chrome::file_badge(&name, theme.dark);
        let text_color = if i == active { theme.tab_text_active } else { theme.tab_text };
        let cells = 2 + 2 + 1 + name.chars().count() + TAB_CLOSE_CELLS;
        let w = cells as f32 * cw;
        spans.push((x, x + w));
        x += w;
        labels.push(("  ".to_string(), text_color));
        labels.push((badge.to_string(), if i == active { badge_color } else { badge_color.with_alpha(0xB0) }));
        labels.push((format!(" {name}{}", " ".repeat(TAB_CLOSE_CELLS)), text_color));
    }
    (labels, spans)
}

/// What is being dragged with the mouse.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Dragged {
    Panel(Panel),
    /// An editor tab (index in the focused half).
    Tab(usize),
}

/// A panel or editor tab being dragged: where the press was, whether it moved enough to count
/// as a drag, the compass targets and the one under the mouse.
struct PanelDrag {
    what: Dragged,
    from: (f32, f32),
    moved: bool,
    targets: Vec<compass::Target>,
    target: Option<compass::Target>,
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
    /// A right-click menu.
    Context(ctxmenu::ContextMenu),
    /// Name for a rename or a new file/folder in the explorer.
    ExplorerPrompt { prompt: menu::Prompt, op: ExplorerOp, target: PathBuf },
    /// "Move … to the recycle bin?"
    ConfirmDelete(menu::Menu, PathBuf),
    /// "Wyven x.y is available – install?"
    Update(menu::Menu),
    /// The color theme picker (state in `State::theme_picker`).
    Themes(menu::Menu),
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
    /// The icon column at the left window edge.
    activity: Rect,
    /// The card the (focused) editor half sits on: its tab bar and text (and preview).
    editor_card: Rect,
    /// The other half's card, when split.
    editor2_card: Option<Rect>,
    /// Space between the cards and their corner radius.
    gap: f32,
    radius: f32,
    /// The dock areas (with their panel tab strip), when open.
    left: Option<Rect>,
    right: Option<Rect>,
    bottom: Option<Rect>,
    tabs: Rect,
    editor: Rect,
    /// Markdown preview, right of the editor.
    preview: Option<Rect>,
    /// The other editor half of a split: its tab bar and text.
    tabs2: Option<Rect>,
    editor2: Option<Rect>,
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
    /// The tab under the mouse, and the icons drawn on tabs (close ×, unsaved ●).
    tab_hover: Option<usize>,
    tab_icons: (Buffer, Buffer),

    explorer: Option<Explorer>,
    /// Where the panels are docked and which ones show.
    dock: dock::Dock,
    dock_bar: dock::DockBar,
    /// The icons at the left window edge.
    activity: chrome::ActivityBar,
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
    theme_picker: Option<ThemePicker>,
    plugins: plugins::Plugins,
    /// Organize usings in two steps (C#): 1 = remove unused, 2 = sort.
    organize_step: Option<u8>,
    /// Zen mode: only the text, fullscreen; and when Esc was last pressed (twice leaves).
    zen: bool,
    last_escape: Option<Instant>,
    split: Option<Split>,
    /// Shortcuts from keymap.toml.
    custom_keys: custom_keys::CustomKeys,
    /// A running update check or download.
    update_rx: Option<std::sync::mpsc::Receiver<update::UpdateEvent>>,
    /// The newer version on offer, and whether the check was asked for.
    update_offer: Option<update::Manifest>,
    update_manual: bool,
    /// The `.code-workspace` file the workspace came from (folders are written back to it).
    workspace_file: Option<PathBuf>,
    /// The file Git last followed (the panel shows the repository of the active file).
    git_followed: Option<PathBuf>,

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
        let icon = winit::window::Icon::from_rgba(include_bytes!("../../../assets/icon-64.rgba").to_vec(), 64, 64).ok();
        let mut attrs = Window::default_attributes()
            .with_title("Wyven")
            .with_window_icon(icon)
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
            Some(p) if workspace::is_workspace_file(&p) => state.open_folder(&p),
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
        match self.demo.take().as_deref() {
            Some("--compass") => {
                if let Some(r) = state.layout.left {
                    let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                    state.panel_drag = Some(PanelDrag { what: Dragged::Panel(Panel::Terminal), from: (0.0, 0.0), moved: false, targets: Vec::new(), target: None });
                    state.panel_drag_move(x, y);
                }
            }
            Some("--menu") => {
                if let Some(r) = state.explorer.as_ref().and(state.layout.left) {
                    let (x, y) = (r.x + r.w * 0.4, r.y + state.ui_font.line_h * 6.3);
                    state.open_explorer_menu(x, y);
                }
            }
            _ => {}
        }
        if let Some(g) = self.open_popup.take() {
            let group: &'static str = leader::items("").iter().find_map(|i| match i.action {
                leader::Action::Group(name) if name == g => Some(name),
                _ => None,
            }).unwrap_or("");
            state.overlay = Overlay::Leader { group, since: Instant::now(), shown: true };
        }
        if self.open_split {
            state.split_editor(true);
        }
        if self.open_zen {
            state.toggle_zen();
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
                    let roots = ex.roots().to_vec();
                    state.index.rebuild(&roots);
                }
                state.window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => state.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => state.key_event(&event),
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x as f32, position.y as f32);
                if let Some(t) = &state.tooltip
                    && state.activity.hover.is_none()
                    && state.dock_bar.hover.is_none()
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
                if let Overlay::Context(menu) = &mut state.overlay {
                    let hover = menu.row_at(x, y);
                    if hover != menu.hover {
                        menu.hover = hover;
                        state.window.request_redraw();
                    }
                }
                // The explorer row under the mouse.
                let explorer_shown = state.panel_shown(Panel::Explorer);
                if let Some(ex) = &mut state.explorer {
                    let hover = (explorer_shown && ex.contains(x, y)).then(|| ex.row_at(y)).flatten();
                    if hover != ex.hover {
                        ex.hover = hover;
                        state.window.request_redraw();
                    }
                }
                // The tab under the mouse (shows its ×).
                let t = state.layout.tabs;
                let tab_hover = (y >= t.y && y < t.y + t.h)
                    .then(|| state.tab_spans.iter().position(|&(a, b)| x >= a && x < b))
                    .flatten();
                if tab_hover != state.tab_hover || tab_hover.is_some() {
                    state.tab_hover = tab_hover;
                    state.window.request_redraw();
                }
                // Header buttons: highlight and a tooltip.
                let header_hover = match state.dock_bar.hit(x, y) {
                    Some(dock::HeaderHit::Button(a, _, b)) => Some((a, b)),
                    _ => None,
                };
                if header_hover != state.dock_bar.hover {
                    state.dock_bar.hover = header_hover;
                    state.tooltip = header_hover.and_then(|(a, b)| {
                        let r = state.dock_bar.buttons.iter().find(|x| x.0 == a && x.2 == b)?.3;
                        let fs = state.renderer.font_system();
                        Some(completion::Tooltip::new(fs, state.ui_font, &state.family, &state.theme, b.tooltip(), r.x, r.y + r.h + 4.0))
                    });
                    state.window.request_redraw();
                }
                // The activity bar: highlight and a tooltip with the shortcut.
                let hover = state.activity.hit(x, y);
                if hover != state.activity.hover {
                    state.activity.hover = hover;
                    state.tooltip = hover.map(|item| {
                        let r = state.activity.slots.iter().find(|s| s.0 == item).map(|s| s.1).unwrap_or_default();
                        let fs = state.renderer.font_system();
                        completion::Tooltip::new(fs, state.ui_font, &state.family, &state.theme, item.tooltip(), r.x + r.w + 4.0, r.y + r.h * 0.2)
                    });
                    state.window.request_redraw();
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
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
                UserEvent::Update => {
                    if state.update_poll() {
                        state.debugger.stop();
                        for server in state.servers.values_mut() {
                            server.shutdown();
                        }
                        event_loop.exit();
                    }
                }
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

/// A file or folder name Windows accepts.
fn valid_file_name(name: &str) -> Result<(), String> {
    const BAD: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "LPT1", "LPT2", "LPT3"];
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty() || name == "." || name == ".." {
        Err("Please enter a name".into())
    } else if let Some(c) = name.chars().find(|c| BAD.contains(c) || c.is_control()) {
        Err(format!("A name cannot contain {c}"))
    } else if name.ends_with(['.', ' ']) {
        Err("A name cannot end with a dot or a space".into())
    } else if reserved.contains(&stem.as_str()) {
        Err(format!("{name} is reserved by Windows"))
    } else {
        Ok(())
    }
}

/// The block of `using …;` lines at the top of a C# file (char range), if there is one.
fn csharp_usings(rope: &wyven_buffer::Rope) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = None;
    for (i, line) in rope.lines().enumerate() {
        let t = line.to_string();
        let t = t.trim();
        let is_using = (t.starts_with("using ") || t.starts_with("global using ")) && t.ends_with(';') && !t.contains('(') && !t.contains(" var ");
        if is_using {
            first.get_or_insert(i);
            last = Some(i);
        } else if !(t.is_empty() || t.starts_with("//") || t.starts_with('#')) {
            break;
        }
    }
    let (first, last) = (first?, last?);
    let end_line = (last + 1).min(rope.len_lines().saturating_sub(1));
    let end = if last + 1 < rope.len_lines() { rope.line_to_char(end_line) } else { rope.len_chars() };
    Some((rope.line_to_char(first), end))
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
        let tab_icons = Self::make_tab_icons(fs, ui_font);
        let activity = chrome::ActivityBar::new(fs, ui_font);
        let settings_page = settings_page::SettingsPage::new(ui_font, family.clone());
        let find_bar = find::FindBar::new(fs, ui_font);
        let (initial_theme, theme_error) = match Theme::resolve(&settings.theme) {
            Ok(t) => (t, None),
            Err(e) => (Theme::dark(), Some(e)),
        };
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
            theme: initial_theme,
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
            tab_hover: None,
            tab_icons,
            explorer: None,
            dock: dock::Dock::load(),
            dock_bar,
            activity,
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
            theme_picker: None,
            plugins: plugins::Plugins::new(),
            organize_step: None,
            zen: false,
            last_escape: None,
            split: None,
            custom_keys: custom_keys::CustomKeys::default(),
            update_rx: None,
            update_offer: None,
            update_manual: false,
            workspace_file: None,
            git_followed: None,
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
        state.reload_plugins(false);
        state.reload_keymap(false);
        state.check_updates(false);
        if let Some(e) = theme_error {
            state.show(e);
        }
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
        let scale = self.renderer.scale_factor();
        let zen = self.zen;
        let status_h = if zen { 0.0 } else { (self.ui_font.line_h * 1.3).round() };
        let tabs_h = if zen { 0.0 } else { (self.ui_font.line_h * 1.75).round() };
        let body_h = h - status_h;
        // Cards on a frame (Visual Studio 2026 style): a gap around and between them.
        let act_w = if zen { 0.0 } else { (self.ui_font.line_h * 2.3).round() };
        let gap = if zen { 0.0 } else { (6.0 * scale).round() };
        let half = (gap / 2.0).round();
        let radius = (7.0 * scale).round();
        let (ox, oy) = (act_w + half, half);
        let (rw, rh) = (w - act_w - gap, body_h - gap);
        let cell = self.ui_font.cell_w;
        let side_w = |me: &Self, a: Area| -> f32 {
            if zen || !me.area_open(a) {
                return 0.0;
            }
            let cells = me.dock.area(a).size.unwrap_or(me.settings.ui.explorer_width);
            (cells * cell).clamp(cell * 12.0, rw * 0.45).round()
        };
        let left_w = side_w(self, Area::Left);
        let right_w = side_w(self, Area::Right).min(rw - left_w - cell * 20.0).max(0.0);
        let center_w = rw - left_w - right_w;
        let bottom_h = if !zen && self.area_open(Area::Bottom) {
            let pct = self.dock.area(Area::Bottom).size.unwrap_or(self.settings.terminal.height);
            (rh * pct / 100.0).round().max(self.font.line_h * 4.0).min(rh - tabs_h - self.font.line_h * 3.0)
        } else {
            0.0
        };
        let deflate = |r: Rect| Rect { x: r.x + half, y: r.y + half, w: (r.w - gap).max(0.0), h: (r.h - gap).max(0.0) };
        let editor_card = if zen {
            let col = (self.font.cell_w * 104.0).min(w);
            Rect { x: ((w - col) / 2.0).round(), y: 0.0, w: col, h: body_h }
        } else {
            deflate(Rect { x: ox + left_w, y: oy, w: center_w, h: rh - bottom_h })
        };
        // A card's tab bar on top, the text below, and the Markdown preview on the right.
        let preview_on = self.preview_visible();
        let fill = |card: Rect, with_preview: bool| -> (Rect, Rect, Option<Rect>) {
            let tabs = Rect { x: card.x, y: card.y, w: card.w, h: tabs_h };
            let body = Rect { x: card.x, y: card.y + tabs_h, w: card.w, h: card.h - tabs_h };
            if with_preview {
                let pw = (body.w / 2.0).round();
                (tabs, Rect { w: body.w - pw, ..body }, Some(Rect { x: body.x + body.w - pw, w: pw, ..body }))
            } else {
                (tabs, body, None)
            }
        };
        let (tabs, editor, preview) = fill(editor_card, preview_on);
        self.layout = Layout {
            activity: Rect { x: 0.0, y: 0.0, w: act_w, h: body_h },
            editor_card,
            editor2_card: None,
            gap,
            radius,
            left: (left_w > 0.0).then(|| deflate(Rect { x: ox, y: oy, w: left_w, h: rh })),
            right: (right_w > 0.0).then(|| deflate(Rect { x: ox + rw - right_w, y: oy, w: right_w, h: rh })),
            bottom: (bottom_h > 0.0).then(|| deflate(Rect { x: ox + left_w, y: oy + rh - bottom_h, w: center_w, h: bottom_h })),
            tabs,
            editor,
            preview,
            tabs2: None,
            editor2: None,
            status: Rect { x: 0.0, y: body_h, w, h: status_h },
        };
        // Split: two cards side by side or stacked; the focused half uses `tabs`/`editor`.
        if let Some(sp) = &self.split
            && !zen
        {
            let c = editor_card;
            let (first, second) = if sp.vertical {
                let w1 = ((c.w - gap) / 2.0).round();
                (Rect { w: w1, ..c }, Rect { x: c.x + w1 + gap, w: c.w - w1 - gap, ..c })
            } else {
                let h1 = ((c.h - gap) / 2.0).round();
                (Rect { h: h1, ..c }, Rect { y: c.y + h1 + gap, h: c.h - h1 - gap, ..c })
            };
            let (mine, other) = if sp.focused_first { (first, second) } else { (second, first) };
            let (t, e, p) = fill(mine, preview_on);
            let (t2, e2, _) = fill(other, false);
            self.layout.editor_card = mine;
            self.layout.editor2_card = Some(other);
            self.layout.tabs = t;
            self.layout.editor = e;
            self.layout.preview = p;
            self.layout.tabs2 = Some(t2);
            self.layout.editor2 = Some(e2);
        }
        self.activity.place(self.layout.activity);
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
        let strips: Vec<(Area, Rect, Vec<Panel>, Option<Panel>)> = Area::ALL
            .into_iter()
            .filter_map(|a| self.layout_area(a).map(|r| (a, r, self.dock.area(a).panels.clone(), self.dock.area(a).active)))
            .collect();
        self.dock_bar.place(&strips);
        if let (Some(sp), Some(r)) = (&mut self.split, self.layout.editor2) {
            let scale = self.renderer.scale_factor();
            for t in &mut sp.tabs {
                t.set_area(r);
                t.scale = scale;
            }
        }
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

    /// Alt → v → z: everything but the text disappears, the window goes fullscreen.
    fn toggle_zen(&mut self) {
        self.zen = !self.zen;
        self.window.set_fullscreen(self.zen.then_some(winit::window::Fullscreen::Borderless(None)));
        if self.zen {
            self.focus = Focus::Editor;
        }
        let mut tabs = std::mem::take(&mut self.tabs);
        for t in &mut tabs {
            self.configure_view(t);
        }
        self.tabs = tabs;
        self.relayout();
        self.show(if self.zen { "Zen mode: Alt → v → z or Esc twice leaves" } else { "Zen mode off" });
    }

    /// The tab of the focused half that shows the same file as `view`.
    fn tab_for_view(&self, view: &EditorView) -> Option<usize> {
        view.doc.path().and_then(|p| self.tab_for(p))
    }

    /// Alt → w → v / h: a second editor half with the current file (or the other direction).
    fn split_editor(&mut self, vertical: bool) {
        if let Some(sp) = &mut self.split {
            sp.vertical = vertical;
            self.relayout();
            return;
        }
        self.diff = None;
        let fs = self.renderer.font_system();
        let twin = self.tabs[self.active].twin(fs);
        let mut tab_bar = Buffer::new(fs, Metrics::new(self.ui_font.size, self.ui_font.line_h));
        tab_bar.set_wrap(Wrap::None);
        self.split = Some(Split {
            vertical,
            tabs: vec![twin],
            active: 0,
            mru: vec![0],
            focused_first: true,
            tab_bar,
            tab_spans: Vec::new(),
        });
        // The new half gets the focus.
        self.focus_other_half();
        self.show("Split: Alt → w → o or ←↑→↓ switches halves, Alt → w → c closes");
    }

    /// Moves the focus to the other editor half.
    fn focus_other_half(&mut self) {
        let Some(sp) = &mut self.split else {
            self.show("No split: Alt → w → v splits the editor");
            return;
        };
        std::mem::swap(&mut self.tabs, &mut sp.tabs);
        std::mem::swap(&mut self.active, &mut sp.active);
        std::mem::swap(&mut self.mru, &mut sp.mru);
        std::mem::swap(&mut self.tab_bar, &mut sp.tab_bar);
        sp.focused_first = !sp.focused_first;
        self.cycling = None;
        self.diff = None;
        self.completion = None;
        self.focus = Focus::Editor;
        self.relayout();
        self.lsp_open_all();
        self.follow_git(false);
        self.window.request_redraw();
    }

    /// Alt → w → ←/↑ (first half) or →/↓ (second half).
    fn focus_half(&mut self, first: bool) {
        if self.split.as_ref().is_some_and(|sp| sp.focused_first != first) {
            self.focus_other_half();
        }
    }

    /// Alt → w → c: one editor again; files only open in the other half come along as tabs.
    fn close_split(&mut self) {
        let Some(sp) = self.split.take() else {
            self.show("No split open");
            return;
        };
        for t in sp.tabs {
            if t.doc.path().is_some() && self.tab_for_view(&t).is_some() {
                continue; // the same file is open here already
            }
            if t.doc.path().is_none() && !t.doc.is_dirty() && t.is_untouched() {
                continue;
            }
            self.tabs.push(t);
            self.mru.push(self.tabs.len() - 1);
        }
        self.relayout();
        self.lsp_open_all();
        self.show("Split closed");
    }

    /// After an edit in the focused half, its file's view in the other half takes the text.
    fn sync_split_twins(&mut self) {
        let Some(sp) = &mut self.split else { return };
        let ed = &self.tabs[self.active];
        let Some(path) = ed.doc.path() else { return };
        let path = path.to_string_lossy().to_lowercase();
        for t in &mut sp.tabs {
            let same = t.doc.path().is_some_and(|p| p.to_string_lossy().to_lowercase() == path);
            if same && (t.doc.version() != ed.doc.version() || t.doc.is_dirty() != ed.doc.is_dirty() || t.doc.text() != ed.doc.text()) {
                t.mirror_from(ed);
            }
        }
    }

    /// The × and ● drawn on tabs.
    fn make_tab_icons(fs: &mut FontSystem, ui: FontMetrics) -> (Buffer, Buffer) {
        let family = chrome::icon_family(fs);
        let size = (ui.size * 0.8).round();
        let close = chrome::label(fs, &chrome::ICON_CLOSE.to_string(), &family, size, size * 1.3);
        let dot = chrome::label(fs, "\u{25CF}", &family, size, size * 1.3);
        (close, dot)
    }

    fn show_context(&mut self, b: ctxmenu::MenuBuilder, x: f32, y: f32) {
        let (w, h) = self.renderer.size();
        let fs = self.renderer.font_system();
        let menu = b.build(fs, self.ui_font, &self.family, x, y, (w as f32, h as f32));
        self.overlay = Overlay::Context(menu);
        self.tooltip = None;
        self.window.request_redraw();
    }

    /// Right-click in the explorer: on a row its commands, on empty space new file/folder.
    fn open_explorer_menu(&mut self, x: f32, y: f32) {
        use ctxmenu::CtxAction as C;
        let Some(ex) = &mut self.explorer else { return };
        self.focus = Focus::Explorer;
        let row = ex.row_at(y);
        if let Some(i) = row {
            ex.selected = i;
            ex.invalidate();
        }
        let row = row.and_then(|_| ex.selected_row().cloned());
        let root = ex.root().to_path_buf();
        let b = match row {
            Some(r) => {
                let dir = if r.is_dir { r.path.clone() } else { r.path.parent().map(Path::to_path_buf).unwrap_or(root) };
                let mut b = ctxmenu::MenuBuilder::new()
                    .item("New File …", "Ctrl+N", C::NewFile(dir.clone()))
                    .item("New Folder …", "Ctrl+Shift+N", C::NewFolder(dir.clone()))
                    .separator();
                if !r.is_root {
                    b = b
                        .item("Rename …", "F2", C::Rename(r.path.clone()))
                        .item("Delete", "Del", C::Delete(r.path.clone()))
                        .separator();
                }
                b = b
                    .item("Copy Path", "", C::CopyPath(r.path.clone()))
                    .item("Copy Relative Path", "", C::CopyRelativePath(r.path.clone()))
                    .separator()
                    .item("Reveal in File Explorer", "", C::RevealInOs(r.path.clone()))
                    .item("Open in Terminal", "", C::OpenTerminalHere(dir));
                if r.is_root && self.roots().len() > 1 {
                    b = b.separator().item("Remove Folder from Workspace", "", C::RemoveRoot(r.path));
                }
                b
            }
            None => ctxmenu::MenuBuilder::new()
                .item("New File …", "Ctrl+N", C::NewFile(root.clone()))
                .item("New Folder …", "Ctrl+Shift+N", C::NewFolder(root.clone()))
                .separator()
                .item("Refresh", "", C::RefreshExplorer)
                .item("Collapse Folders", "", C::CollapseExplorer)
                .separator()
                .item("Open in Terminal", "", C::OpenTerminalHere(root)),
        };
        self.show_context(b, x, y);
    }

    /// Right-click on a tab.
    fn open_tab_menu(&mut self, i: usize, x: f32, y: f32) {
        use ctxmenu::CtxAction as C;
        let path = self.tabs[i].doc.path().map(Path::to_path_buf);
        let mut b = ctxmenu::MenuBuilder::new()
            .item("Close", "Ctrl+W", C::CloseTab(i))
            .item("Close Others", "", C::CloseOthers(i))
            .item("Close to the Right", "", C::CloseToRight(i))
            .item("Close All", "Alt f c", C::CloseAll)
            .separator();
        if let Some(p) = path {
            b = b
                .item("Copy Path", "", C::CopyPath(p.clone()))
                .item("Copy Relative Path", "", C::CopyRelativePath(p.clone()))
                .item("Reveal in Explorer", "", C::RevealInExplorer(p.clone()))
                .item("Reveal in File Explorer", "", C::RevealInOs(p))
                .separator();
        }
        b = b.item("Split Right", "Alt w v", C::SplitRight).item("Split Down", "Alt w h", C::SplitDown);
        self.activate(i);
        self.show_context(b, x, y);
    }

    /// Right-click in the text: the click moves the cursor unless it is inside the selection.
    fn open_editor_menu(&mut self, x: f32, y: f32) {
        use ctxmenu::CtxAction as C;
        use keymap::Command as K;
        self.focus = Focus::Editor;
        let pos = self.editor().hit_test(x, y);
        let sel = self.editor().doc.selection().primary();
        if pos < sel.from() || pos > sel.to() || sel.is_empty() {
            self.tabs[self.active].doc.set_cursor(pos);
        }
        let mut b = ctxmenu::MenuBuilder::new();
        if self.editor().lsp_kind().is_some() {
            b = b
                .item("Go to Definition", "F12", C::Run(K::GotoDefinition))
                .item("Go to Implementation", "Ctrl+F12", C::Run(K::GotoImplementation))
                .item("Find All References", "Shift+F12", C::Run(K::FindReferences))
                .separator()
                .item("Rename Symbol …", "F2", C::Run(K::Rename))
                .item("Quick Actions …", "Ctrl+.", C::Run(K::QuickActions))
                .item("Format Document", "Ctrl+K Ctrl+D", C::Run(K::Format))
                .separator();
        }
        b = b
            .item("Cut", "Ctrl+X", C::Run(K::Cut))
            .item("Copy", "Ctrl+C", C::Run(K::Copy))
            .item("Paste", "Ctrl+V", C::Run(K::Paste))
            .separator()
            .item("Select All", "Ctrl+A", C::Run(K::SelectAll));
        self.show_context(b, x, y);
    }

    fn context_key(&mut self, key: &Key) {
        let Overlay::Context(menu) = &mut self.overlay else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.overlay = Overlay::None,
            Key::Named(NamedKey::ArrowUp) => menu.move_hover(-1),
            Key::Named(NamedKey::ArrowDown) => menu.move_hover(1),
            Key::Named(NamedKey::Enter) => {
                let action = menu.hover.and_then(|h| menu.action(h));
                self.overlay = Overlay::None;
                if let Some(a) = action {
                    self.run_ctx(a);
                }
            }
            _ => {}
        }
        self.window.request_redraw();
    }

    /// Runs a menu row.
    fn run_ctx(&mut self, action: ctxmenu::CtxAction) {
        use ctxmenu::CtxAction as C;
        let is_new_file = matches!(action, C::NewFile(_));
        let only_right = matches!(action, C::CloseToRight(_));
        match action {
            C::NewFile(dir) | C::NewFolder(dir) => {
                let op = if is_new_file { ExplorerOp::NewFile } else { ExplorerOp::NewFolder };
                let what = if op == ExplorerOp::NewFile { "New file" } else { "New folder" };
                let title = format!("{what} in {} (Enter, Esc)", file_name(&dir));
                let prompt = menu::Prompt::new(self.renderer.font_system(), self.ui_font, &title, "");
                self.overlay = Overlay::ExplorerPrompt { prompt, op, target: dir };
            }
            C::Rename(p) => {
                let name = file_name(&p);
                let title = format!("Rename {name} (Enter, Esc)");
                let prompt = menu::Prompt::new(self.renderer.font_system(), self.ui_font, &title, &name);
                self.overlay = Overlay::ExplorerPrompt { prompt, op: ExplorerOp::Rename, target: p };
            }
            C::Delete(p) => {
                let is_dir = p.is_dir();
                let title = format!("Move {}{} to the recycle bin?", file_name(&p), if is_dir { " and everything in it" } else { "" });
                let items = vec![
                    ("Yes, move to recycle bin".to_string(), wyven_lsp::Value::Bool(true)),
                    ("No".to_string(), wyven_lsp::Value::Bool(false)),
                ];
                let mut m = menu::Menu::new(self.renderer.font_system(), self.ui_font, &title, items);
                m.select(1);
                self.overlay = Overlay::ConfirmDelete(m, p);
            }
            C::CopyPath(p) => {
                self.set_clipboard((p.display().to_string(), false));
                self.show("Path copied");
            }
            C::CopyRelativePath(p) => {
                let rel = self
                    .root_for(Some(&p))
                    .and_then(|r| p.strip_prefix(&r).ok().map(Path::to_path_buf))
                    .unwrap_or(p);
                self.set_clipboard((rel.display().to_string().replace('\\', "/"), false));
                self.show("Relative path copied");
            }
            C::RevealInOs(p) => {
                let arg = if p.is_dir() { p.display().to_string() } else { format!("/select,{}", p.display()) };
                if let Err(e) = std::process::Command::new("explorer.exe").arg(arg).spawn() {
                    self.show(format!("File Explorer: {e}"));
                }
            }
            C::OpenTerminalHere(dir) => {
                self.dock.show(Panel::Terminal);
                self.focus = Focus::Terminal;
                self.dock_changed();
                if self.terminal.term.is_none() {
                    self.start_terminal();
                }
                if let Some(t) = &self.terminal.term {
                    let cmd_exe = self.settings.terminal.shell.to_lowercase().contains("cmd");
                    let line = if cmd_exe { format!("cd /d \"{}\"\r", dir.display()) } else { format!("cd \"{}\"\r", dir.display()) };
                    t.type_text(&line, false);
                }
            }
            C::RemoveRoot(p) => self.remove_folder(&p),
            C::RefreshExplorer => self.after_files_changed(),
            C::CollapseExplorer => {
                if let Some(ex) = &mut self.explorer {
                    ex.collapse_all();
                }
            }
            C::CloseTab(i) => self.close_tab(i),
            C::CloseOthers(keep) | C::CloseToRight(keep) => {
                let mut kept = 0;
                let mut i = self.tabs.len();
                let mut keep = keep;
                while i > 0 {
                    i -= 1;
                    if i == keep || (only_right && i < keep) {
                        continue;
                    }
                    if self.tabs[i].doc.is_dirty() {
                        kept += 1;
                        continue;
                    }
                    self.armed = None;
                    self.close_tab(i);
                    if i < keep {
                        keep -= 1;
                    }
                }
                if kept > 0 {
                    self.show(format!("{kept} file(s) with unsaved changes stay open."));
                }
            }
            C::CloseAll => self.close_all(),
            C::RevealInExplorer(p) => {
                if let Some(ex) = &mut self.explorer {
                    ex.reveal(&p);
                }
                self.dock.show(Panel::Explorer);
                self.focus = Focus::Explorer;
                self.dock_changed();
            }
            C::SplitRight => self.split_editor(true),
            C::SplitDown => self.split_editor(false),
            C::Run(cmd) => self.run(cmd),
        }
        self.window.request_redraw();
    }

    /// A click on an icon button in a panel header.
    fn header_button(&mut self, panel: Panel, button: dock::HeaderButton) {
        use dock::HeaderButton as B;
        match button {
            B::NewFile | B::NewFolder => {
                // In the selected folder, or at the top of the main folder.
                self.focus = Focus::Explorer;
                let op = if button == B::NewFile { ExplorerOp::NewFile } else { ExplorerOp::NewFolder };
                self.start_explorer_op(op);
            }
            B::Refresh => match panel {
                Panel::Git => self.git.refresh_status(),
                _ => self.after_files_changed(),
            },
            B::CollapseAll => {
                if let Some(ex) = &mut self.explorer {
                    ex.collapse_all();
                }
            }
            B::Close => {
                self.dock.hide(panel);
                if self.focused_panel() == Some(panel) {
                    self.focus = Focus::Editor;
                }
                self.dock_changed();
            }
        }
        self.window.request_redraw();
    }

    /// A click on the activity bar: shows the panel (or hides it when it is already in front).
    fn activity_click(&mut self, item: chrome::ActItem) {
        match item {
            chrome::ActItem::Settings => self.open_settings_page(),
            chrome::ActItem::Panel(p) => {
                if self.panel_shown(p) && self.focused_panel() == Some(p) {
                    self.dock.hide(p);
                    self.focus = Focus::Editor;
                    self.dock_changed();
                } else {
                    self.activate_panel(p);
                }
            }
        }
        self.window.request_redraw();
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
        let grab = 2.0 * self.renderer.scale_factor() + self.layout.gap / 2.0;
        let half = self.layout.gap / 2.0;
        let near = |a: f32, b: f32| (a - b).abs() <= grab;
        if let Some(r) = self.layout.left
            && near(x, r.x + r.w + half)
            && y >= r.y
            && y < r.y + r.h
        {
            return Some(Area::Left);
        }
        if let Some(r) = self.layout.right
            && near(x, r.x - half)
            && y >= r.y
            && y < r.y + r.h
        {
            return Some(Area::Right);
        }
        if let Some(r) = self.layout.bottom
            && near(y, r.y - half)
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
        // Sizes count from the outer edge of the content (right of the activity bar).
        let left0 = self.layout.activity.w + self.layout.gap / 2.0;
        let right0 = w - self.layout.gap / 2.0;
        let rh = body_h - self.layout.gap;
        let size = match a {
            Area::Left => ((x - left0).clamp(cell * 12.0, w * 0.45)) / cell,
            Area::Right => ((right0 - x).clamp(cell * 12.0, w * 0.45)) / cell,
            Area::Bottom => ((body_h - self.layout.gap / 2.0 - y) / rh * 100.0).clamp(10.0, 80.0),
        };
        self.dock.area_mut(a).size = Some((size * 10.0).round() / 10.0);
        self.relayout();
    }

    /// Mouse moves while a panel tab is held: start the drag, follow the drop target.
    fn panel_drag_move(&mut self, x: f32, y: f32) {
        let scale = self.renderer.scale_factor();
        let Some(what) = self.panel_drag.as_ref().map(|d| d.what) else { return };
        let content = Rect {
            x: self.layout.activity.w,
            y: 0.0,
            w: self.layout.status.w - self.layout.activity.w,
            h: self.layout.status.y,
        };
        let areas: Vec<(Area, Rect)> = Area::ALL.into_iter().filter_map(|a| self.layout_area(a).map(|r| (a, r))).collect();
        let inside = |r: &Rect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        let under = areas
            .iter()
            .find(|(_, r)| inside(r))
            .map(|(a, r)| (*r, Some(*a)))
            .or_else(|| [Some(self.layout.editor_card), self.layout.editor2_card].into_iter().flatten().find(|r| inside(r)).map(|r| (r, None)));
        let size = (self.ui_font.line_h * 2.1).round();
        let targets = match what {
            Dragged::Panel(_) => compass::panel_targets(content, &areas, under, size),
            Dragged::Tab(_) => compass::tab_targets(self.layout.editor_card, size),
        };
        // Ctrl: move without docking (like Visual Studio).
        let ctrl = self.modifiers.control_key();
        let Some(d) = &mut self.panel_drag else { return };
        let dist = ((x - d.from.0).powi(2) + (y - d.from.1).powi(2)).sqrt();
        if !d.moved && dist < 8.0 * scale {
            return;
        }
        d.moved = true;
        d.target = if ctrl { None } else { compass::hit(&targets, x, y) };
        d.targets = targets;
        self.window.request_redraw();
    }

    /// Mouse released after dragging a panel or a tab: dock it where the compass says.
    fn panel_drop(&mut self) {
        let Some(d) = self.panel_drag.take() else { return };
        if !d.moved {
            return;
        }
        match (d.what, d.target) {
            (Dragged::Panel(p), Some(t)) => {
                if let Some(area) = t.area {
                    self.dock.move_panel(p, area, usize::MAX);
                    self.dock_changed();
                    self.activate_panel(p);
                }
            }
            (Dragged::Tab(i), Some(t)) => self.split_tab(i, t.side),
            _ => {}
        }
        self.window.request_redraw();
    }

    /// Takes tab `i` out of a tab list, keeping `active` and the recently used order right.
    fn take_tab(tabs: &mut Vec<EditorView>, active: &mut usize, mru: &mut Vec<usize>, i: usize) -> EditorView {
        let t = tabs.remove(i);
        mru.retain(|&m| m != i);
        for m in mru.iter_mut() {
            if *m > i {
                *m -= 1;
            }
        }
        if *active > i || *active >= tabs.len() {
            *active = active.saturating_sub(1);
        }
        t
    }

    /// A tab dropped on a compass target: the editor splits and the tab moves to that side.
    fn split_tab(&mut self, i: usize, side: compass::Side) {
        use compass::Side;
        if side == Side::Center || i >= self.tabs.len() {
            return;
        }
        let vertical = matches!(side, Side::Left | Side::Right);
        let first = matches!(side, Side::Left | Side::Top);
        if self.split.is_none() {
            self.activate(i);
            self.split_editor(vertical);
            // The tab moves (the old half keeps its other tabs).
            if let Some(sp) = &mut self.split {
                sp.focused_first = first;
                if sp.tabs.len() > 1 {
                    Self::take_tab(&mut sp.tabs, &mut sp.active, &mut sp.mru, i);
                }
            }
        } else {
            if self.tabs.len() > 1 {
                let t = Self::take_tab(&mut self.tabs, &mut self.active, &mut self.mru, i);
                self.focus_other_half();
                self.tabs.push(t);
                self.mru.insert(0, self.tabs.len() - 1);
                self.active = self.tabs.len() - 1;
            } else {
                self.focus_other_half();
            }
            if let Some(sp) = &mut self.split {
                sp.vertical = vertical;
                sp.focused_first = first;
            }
        }
        self.relayout();
        self.lsp_open_all();
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
        if let Some(sp) = &mut self.split {
            for t in &mut sp.tabs {
                t.set_font(self.renderer.font_system(), font);
            }
            sp.tab_bar.set_metrics(Metrics::new(ui.size, ui.line_h));
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
        self.tab_icons = Self::make_tab_icons(self.renderer.font_system(), ui);
        self.activity.set_font(self.renderer.font_system(), ui);
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

    /// Opens a folder (with the folders added to it earlier) or a `.code-workspace` file.
    fn open_folder(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if workspace::is_workspace_file(&path) {
            self.open_workspace_file(&path);
            return;
        }
        let mut roots = vec![path.clone()];
        roots.extend(workspace::extra_folders(&path));
        config::remember_project(&path);
        self.open_roots(roots, None);
    }

    fn open_workspace_file(&mut self, file: &Path) {
        match workspace::read_file(file) {
            Ok(folders) => {
                let existing: Vec<PathBuf> = folders.into_iter().filter(|f| f.is_dir()).collect();
                if existing.is_empty() {
                    self.show(format!("{}: none of its folders exist", file_name(file)));
                    return;
                }
                config::remember_project(file);
                self.open_roots(existing, Some(file.to_path_buf()));
            }
            Err(e) => self.show(e),
        }
    }

    fn workspace_title(file: Option<&Path>) -> Option<String> {
        file.and_then(Path::file_stem).map(|s| format!("{} (Workspace)", s.to_string_lossy()))
    }

    fn open_roots(&mut self, roots: Vec<PathBuf>, file: Option<PathBuf>) {
        let fs = self.renderer.font_system();
        let mut ex = Explorer::new(roots[0].clone(), fs, self.ui_font, self.family.clone());
        for r in &roots[1..] {
            ex.add_root(r.clone());
        }
        ex.set_title(Self::workspace_title(file.as_deref()));
        self.explorer = Some(ex);
        self.workspace_file = file;
        self.dock.show(Panel::Explorer);
        self.index.rebuild(&roots);
        self.search.set_roots(&roots);
        self.follow_git(true);
        self.claude_workspace_changed();
        self.relayout();
    }

    /// What an explorer operation acts on: the explorer's selection while it has the focus,
    /// else the active file.
    fn explorer_target(&self) -> Option<(PathBuf, bool, bool)> {
        if self.focus == Focus::Explorer
            && let Some(row) = self.explorer.as_ref().and_then(|ex| ex.selected_row())
        {
            return Some((row.path.clone(), row.is_dir, row.is_root));
        }
        if let Some(p) = self.editor().doc.path() {
            return Some((p.to_path_buf(), false, false));
        }
        self.project_root().map(|r| (r.to_path_buf(), true, true))
    }

    /// F2 / Del / Ctrl+N / Ctrl+Shift+N in the explorer, Alt → f → r/d/f/k.
    fn start_explorer_op(&mut self, op: ExplorerOp) {
        let Some((path, is_dir, is_root)) = self.explorer_target() else {
            self.show("No file or folder selected");
            return;
        };
        let fs = self.renderer.font_system();
        let name = file_name(&path);
        match op {
            ExplorerOp::Rename | ExplorerOp::Delete if is_root => {
                self.show("That is a workspace folder: remove it with the × or rename it outside Wyven");
            }
            ExplorerOp::Rename => {
                let title = format!("Rename {name} (Enter, Esc)");
                let prompt = menu::Prompt::new(fs, self.ui_font, &title, &name);
                self.overlay = Overlay::ExplorerPrompt { prompt, op, target: path };
            }
            ExplorerOp::Delete => {
                let title = format!("Move {name}{} to the recycle bin?", if is_dir { " and everything in it" } else { "" });
                let items = vec![
                    ("Yes, move to recycle bin".to_string(), wyven_lsp::Value::Bool(true)),
                    ("No".to_string(), wyven_lsp::Value::Bool(false)),
                ];
                let mut menu = menu::Menu::new(fs, self.ui_font, &title, items);
                menu.select(1);
                self.overlay = Overlay::ConfirmDelete(menu, path);
            }
            ExplorerOp::NewFile | ExplorerOp::NewFolder => {
                let dir = if is_dir { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
                let what = if op == ExplorerOp::NewFile { "New file" } else { "New folder" };
                let title = format!("{what} in {} (Enter, Esc)", file_name(&dir));
                let prompt = menu::Prompt::new(fs, self.ui_font, &title, "");
                self.overlay = Overlay::ExplorerPrompt { prompt, op, target: dir };
            }
        }
        self.window.request_redraw();
    }

    fn explorer_prompt_key(&mut self, event: &KeyEvent, key: &Key) {
        let mods = self.modifiers;
        let ctrl = mods.control_key() && !mods.alt_key();
        let Overlay::ExplorerPrompt { prompt, .. } = &mut self.overlay else { return };
        match key {
            Key::Named(NamedKey::Escape) => self.overlay = Overlay::None,
            Key::Named(NamedKey::Enter) => {
                let Overlay::ExplorerPrompt { prompt, op, target } = std::mem::replace(&mut self.overlay, Overlay::None) else {
                    return;
                };
                self.finish_explorer_op(op, &target, prompt.text.trim());
            }
            Key::Named(NamedKey::Backspace) => prompt.backspace(ctrl),
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::End | NamedKey::Home) => prompt.keep(),
            _ => {
                if let Some(t) = Keymap::text(event, mods) {
                    prompt.type_text(&t);
                }
            }
        }
        self.window.request_redraw();
    }

    fn finish_explorer_op(&mut self, op: ExplorerOp, target: &Path, name: &str) {
        if let Err(e) = valid_file_name(name) {
            self.show(e);
            return;
        }
        let result = match op {
            ExplorerOp::Rename => self.rename_on_disk(target, name),
            ExplorerOp::NewFile => {
                let path = target.join(name);
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map(|_| Some(path))
                    .map_err(|e| format!("{name}: {e}"))
            }
            ExplorerOp::NewFolder => {
                let path = target.join(name);
                std::fs::create_dir(&path).map(|_| Some(path)).map_err(|e| format!("{name}: {e}"))
            }
            ExplorerOp::Delete => Ok(None),
        };
        match result {
            Ok(Some(path)) => {
                self.after_files_changed();
                if op == ExplorerOp::NewFile {
                    self.open_path(&path);
                } else if let Some(ex) = &mut self.explorer {
                    ex.reveal(&path);
                }
            }
            Ok(None) => {}
            Err(e) => self.show(e),
        }
    }

    /// Renames on disk; open tabs inside follow the new path.
    fn rename_on_disk(&mut self, from: &Path, name: &str) -> Result<Option<PathBuf>, String> {
        let to = from.parent().map(|p| p.join(name)).ok_or("cannot rename this")?;
        let same_ignoring_case = to.to_string_lossy().to_lowercase() == from.to_string_lossy().to_lowercase();
        if to.exists() && !same_ignoring_case {
            return Err(format!("{name} already exists"));
        }
        std::fs::rename(from, &to).map_err(|e| format!("Rename failed: {e}"))?;
        for i in 0..self.tabs.len() {
            let Some(old) = self.tabs[i].doc.path().map(Path::to_path_buf) else { continue };
            let Ok(rest) = old.strip_prefix(from) else { continue };
            let new = if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) };
            for server in self.servers.values_mut() {
                server.close(&old);
            }
            self.tabs[i].doc.set_path(new);
            self.tabs[i].refresh_disk_mtime();
        }
        self.lsp_open_all();
        self.show(format!("Renamed to {name}"));
        Ok(Some(to))
    }

    fn confirm_delete_key(&mut self, key: &Key) {
        let Overlay::ConfirmDelete(menu, _) = &mut self.overlay else { return };
        let choice = match key {
            Key::Named(NamedKey::Escape) => Some(false),
            Key::Named(NamedKey::ArrowUp) => {
                menu.move_selection(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                menu.move_selection(1);
                None
            }
            Key::Named(NamedKey::Enter) => menu.selected().and_then(|v| v.as_bool()),
            Key::Character(c) if c.as_str() == "1" => Some(true),
            Key::Character(c) if c.as_str() == "2" => Some(false),
            _ => None,
        };
        let Some(yes) = choice else { return };
        let Overlay::ConfirmDelete(_, path) = std::mem::replace(&mut self.overlay, Overlay::None) else { return };
        if yes {
            self.delete_to_trash(&path);
        }
    }

    /// Moves a file or folder to the recycle bin and closes its tabs.
    fn delete_to_trash(&mut self, path: &Path) {
        let inside = |t: &EditorView| t.doc.path().is_some_and(|p| p.starts_with(path));
        if self.tabs.iter().any(|t| inside(t) && t.doc.is_dirty()) {
            self.show("It has unsaved changes open: save or close them first");
            return;
        }
        if let Err(e) = trash::delete(path) {
            self.show(format!("Could not move to the recycle bin: {e}"));
            return;
        }
        while let Some(i) = self.tabs.iter().position(&inside) {
            self.close_tab(i);
        }
        self.after_files_changed();
        self.show(format!("{} moved to the recycle bin", file_name(path)));
    }

    /// Files were created, renamed or deleted: explorer, file index and Git catch up.
    fn after_files_changed(&mut self) {
        if let Some(ex) = &mut self.explorer {
            ex.refresh();
        }
        self.index.rebuild(&self.roots());
        if self.git.repo.is_some() {
            self.git.refresh_status();
        }
        self.window.request_redraw();
    }

    /// Alt → p → a: another folder in this window.
    fn add_folder(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Add Folder to Workspace");
        if let Some(dir) = self.project_root() {
            dialog = dialog.set_directory(dir.parent().unwrap_or(dir));
        }
        let Some(path) = dialog.pick_folder() else { return };
        if self.explorer.is_none() {
            self.open_folder(&path);
            return;
        }
        let added = self.explorer.as_mut().is_some_and(|ex| ex.add_root(path.clone()));
        if !added {
            self.show("That folder is already in the workspace.");
            return;
        }
        self.workspace_changed();
        self.show(format!("Added {} to the workspace", file_name(&path)));
    }

    /// The × on a folder in the explorer.
    fn remove_folder(&mut self, path: &Path) {
        if self.explorer.as_mut().is_some_and(|ex| ex.remove_root(path)) {
            self.workspace_changed();
            self.show(format!("Removed {} from the workspace", file_name(path)));
        }
    }

    /// Folders added or removed: search, Git, Claude Code and the saved workspace follow.
    fn workspace_changed(&mut self) {
        let roots = self.roots();
        if roots.is_empty() {
            return;
        }
        self.index.rebuild(&roots);
        self.search.set_roots(&roots);
        match &self.workspace_file {
            Some(f) => {
                if let Err(e) = workspace::write_file(f, &roots) {
                    self.show(e);
                }
            }
            None => {
                config::remember_project(&roots[0]);
                workspace::remember_extra_folders(&roots[0], &roots[1..]);
            }
        }
        self.claude_workspace_changed();
        self.follow_git(true);
        self.relayout();
    }

    /// Alt → p → w: the folders as a VS Code `.code-workspace` file.
    fn save_workspace_as(&mut self) {
        let roots = self.roots();
        if roots.is_empty() {
            self.show("No folder open: Alt → p → o opens one.");
            return;
        }
        let dialog = rfd::FileDialog::new()
            .set_title("Save Workspace As")
            .add_filter("Workspace", &[workspace::EXTENSION])
            .set_directory(&roots[0])
            .set_file_name(format!("{}.{}", file_name(&roots[0]), workspace::EXTENSION));
        let Some(mut file) = dialog.save_file() else { return };
        if !workspace::is_workspace_file(&file) {
            file.set_extension(workspace::EXTENSION);
        }
        match workspace::write_file(&file, &roots) {
            Ok(()) => {
                config::remember_project(&file);
                if let Some(ex) = &mut self.explorer {
                    ex.set_title(Self::workspace_title(Some(&file)));
                }
                self.show(format!("Workspace saved: {}", file_name(&file)));
                self.workspace_file = Some(file);
            }
            Err(e) => self.show(e),
        }
    }

    /// Alt → p → f.
    fn open_workspace_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Open Workspace").add_filter("Workspace", &[workspace::EXTENSION]);
        if let Some(dir) = self.project_root() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(file) = dialog.pick_file() {
            self.open_folder(&file);
        }
    }

    /// Git shows the repository of the active file (`force`: check again even for the same file).
    fn follow_git(&mut self, force: bool) {
        let path = self.editor().doc.path().map(Path::to_path_buf);
        if !force && path == self.git_followed {
            return;
        }
        self.git_followed = path.clone();
        let inside = |p: &Path, root: &Path| {
            let (p, r) = (p.to_string_lossy().to_lowercase(), root.to_string_lossy().to_lowercase());
            p.starts_with(&r)
        };
        if let (Some(repo), Some(p)) = (&self.git.repo, &path)
            && inside(p, repo.root())
            && !force
        {
            return;
        }
        if !force && path.is_none() && self.git.repo.is_some() {
            return;
        }
        let Some(root) = self.root_for(path.as_deref()) else { return };
        let before = self.git.repo.as_ref().map(|r| r.root().to_path_buf());
        self.git.open_repo(&root);
        let after = self.git.repo.as_ref().map(|r| r.root().to_path_buf());
        if before != after || force {
            for t in &mut self.tabs {
                t.git_base = None;
            }
            self.load_git_heads();
        }
    }

    /// The workspace folders (the first is the main folder).
    fn roots(&self) -> Vec<PathBuf> {
        self.explorer.as_ref().map(|e| e.roots().to_vec()).unwrap_or_default()
    }

    /// The workspace folder that contains `path` (else the main folder).
    fn root_for(&self, path: Option<&Path>) -> Option<PathBuf> {
        let ex = self.explorer.as_ref()?;
        Some(path.and_then(|p| ex.root_of(p)).unwrap_or(ex.root()).to_path_buf())
    }

    /// The workspace folder of the active file.
    fn active_root(&self) -> Option<PathBuf> {
        self.root_for(self.editor().doc.path())
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
            self.plugin_event(plugins::Event::Open);
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
        let switched = index != self.active;
        self.active = index;
        if switched {
            self.plugin_event(plugins::Event::Switch);
        }
        self.diff = None;
        if self.cycling.is_none() {
            self.mru.retain(|&i| i != index);
            self.mru.insert(0, index);
        }
        if let Some(ex) = &mut self.explorer {
            ex.invalidate();
        }
        self.follow_git(false);
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
        if self.tabs.is_empty()
            && let Some(sp) = self.split.take()
        {
            // The half is empty: the other half takes the whole column.
            self.tabs = sp.tabs;
            self.active = sp.active.min(self.tabs.len() - 1);
            self.mru = sp.mru;
            self.relayout();
            self.lsp_open_all();
            return;
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
        self.plugin_event(plugins::Event::BeforeSave);
        let ed = &mut self.tabs[self.active];
        match ed.doc.save() {
            Ok(()) => {
                ed.refresh_disk_mtime();
                let name = ed.file_name();
                if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
                    server.saved(path);
                }
                let is_settings = ed.doc.path().is_some() && ed.doc.path() == settings::path().as_deref();
                let is_keymap = ed.doc.path().is_some() && ed.doc.path() == custom_keys::path().as_deref();
                self.show(format!("{name} saved"));
                self.plugin_event(plugins::Event::Save);
                if is_settings {
                    self.reload_settings();
                }
                if is_keymap {
                    self.reload_keymap(true);
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
        let other = self.split.as_ref().map_or(0, |sp| {
            // A file open in both halves counts once.
            sp.tabs.iter().filter(|t| t.doc.is_dirty() && self.tab_for_view(t).is_none()).count()
        });
        let dirty = self.tabs.iter().filter(|t| t.doc.is_dirty()).count() + other;
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
            Overlay::ExplorerPrompt { .. } => {
                self.explorer_prompt_key(event, &key);
                return;
            }
            Overlay::ConfirmDelete(..) => {
                self.confirm_delete_key(&key);
                return;
            }
            Overlay::Themes(_) => {
                self.theme_picker_key(event, &key);
                return;
            }
            Overlay::Update(_) => {
                self.update_key(&key);
                return;
            }
            Overlay::Context(_) => {
                self.context_key(&key);
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
        if self.zen && key == Key::Named(NamedKey::Escape) {
            let now = Instant::now();
            if self.last_escape.is_some_and(|t| now.duration_since(t) < Duration::from_millis(600)) {
                self.last_escape = None;
                self.toggle_zen();
                return;
            }
            self.last_escape = Some(now);
        }
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
        let explorer_new = mods.control_key()
            && !mods.alt_key()
            && matches!(&key, Key::Character(c) if c.as_str().eq_ignore_ascii_case("n"));
        if self.focus == Focus::Explorer && (explorer_new || (!mods.control_key() && !mods.alt_key()))
            && self.explorer_key(&key) {
                return;
            }

        // Own shortcuts from keymap.toml come first (not in the middle of a Ctrl+K/M chord).
        if self.keymap.chord_pending().is_none()
            && let Some(combo) = custom_keys::combo_of(&key, mods)
            && let Some(binding) = self.custom_keys.key(&combo)
        {
            let binding = binding.map(str::to_string);
            if let Some(cmd) = binding.and_then(|id| self.command_by_id(&id)) {
                self.finish_cycling();
                self.run_palette_cmd(cmd);
            }
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
            Key::Named(NamedKey::F2) => {
                self.start_explorer_op(ExplorerOp::Rename);
                return true;
            }
            Key::Named(NamedKey::Delete) => {
                self.start_explorer_op(ExplorerOp::Delete);
                return true;
            }
            Key::Character(c) if self.modifiers.control_key() && c.as_str().eq_ignore_ascii_case("n") => {
                let op = if self.modifiers.shift_key() { ExplorerOp::NewFolder } else { ExplorerOp::NewFile };
                self.start_explorer_op(op);
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
                .active_root()
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
                lsp::Event::WorkspaceEdit { edit, what } => {
                    self.apply_workspace_edit(&edit, &what);
                    // Organizing usings: after removing the unused ones, sort the rest.
                    if self.organize_step == Some(2) {
                        self.request_organize(2);
                    }
                }
                lsp::Event::Implementations(locs) => match locs.len() {
                    0 => self.show("No implementation found"),
                    1 => {
                        let (path, start) = locs[0].clone();
                        self.open_path(&path);
                        let ed = self.editor_mut();
                        if ed.doc.path() == Some(path.as_path()) {
                            let pos = wyven_lsp::from_lsp(ed.doc.text(), &start);
                            ed.jump_to(pos);
                        }
                    }
                    _ => self.show_locations(locs, "implementation"),
                },
                lsp::Event::OrganizeActions(actions) => self.organize_actions(actions),
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
        view.minimap = if self.zen {
            editor::MinimapMode::parse("off")
        } else {
            editor::MinimapMode::parse(&self.settings.editor.minimap)
        };
        view.sticky_scroll = self.settings.editor.sticky_scroll;
        view.wrap = self.settings.editor.word_wrap;
        view.show_inlay_hints = self.settings.editor.inlay_hints;
        view.show_code_lens = self.settings.editor.code_lens;
        view.scale = self.renderer.scale_factor();
        if !view.doc.indent_detected() {
            view.doc.indent_unit = self.settings.indent_unit();
        }
        view.invalidate();
    }

    /// Looks for a newer version (`manual`: now, and say the result; else at most daily).
    fn check_updates(&mut self, manual: bool) {
        let source = self.settings.update.url.trim().to_string();
        if source.is_empty() {
            if manual {
                self.show("No update address set: Settings (Alt → ,) → Updates");
            }
            return;
        }
        if !manual && (!self.settings.update.check || !update::due()) {
            return;
        }
        if manual {
            self.show("Checking for updates …");
        }
        self.update_manual = manual;
        let proxy = self.proxy.clone();
        self.update_rx = Some(update::check(source, manual, move || {
            let _ = proxy.send_event(UserEvent::Update);
        }));
    }

    /// Results of the update thread. Returns true when Wyven should close for the installer.
    fn update_poll(&mut self) -> bool {
        let Some(rx) = &self.update_rx else { return false };
        let events: Vec<update::UpdateEvent> = rx.try_iter().collect();
        let mut quit = false;
        for ev in events {
            match ev {
                update::UpdateEvent::Checked { found: Some(m), .. } => {
                    let notes = if m.notes.trim().is_empty() { String::new() } else { format!(" – {}", m.notes.trim()) };
                    let title = format!("Wyven {} is available (you have {}){notes}", m.version, update::CURRENT);
                    let items = vec![
                        (format!("Install {} now (Wyven restarts)", m.version), wyven_lsp::Value::Bool(true)),
                        ("Later".to_string(), wyven_lsp::Value::Bool(false)),
                    ];
                    let fs = self.renderer.font_system();
                    self.overlay = Overlay::Update(menu::Menu::new(fs, self.ui_font, &title, items));
                    self.update_offer = Some(m);
                }
                update::UpdateEvent::Checked { found: None, manual } => {
                    if manual {
                        self.show(format!("Wyven is up to date ({})", update::CURRENT));
                    }
                }
                update::UpdateEvent::Ready(path) => match update::run_installer(&path) {
                    Ok(()) => quit = true,
                    Err(e) => self.show(e),
                },
                update::UpdateEvent::Failed(e) => {
                    log::warn!("update: {e}");
                    if self.update_manual || self.update_offer.is_some() {
                        self.show(format!("Update: {e}"));
                    }
                }
            }
        }
        self.window.request_redraw();
        quit
    }

    fn update_key(&mut self, key: &Key) {
        let Overlay::Update(menu) = &mut self.overlay else { return };
        let choice = match key {
            Key::Named(NamedKey::Escape) => Some(false),
            Key::Named(NamedKey::ArrowUp) => {
                menu.move_selection(-1);
                None
            }
            Key::Named(NamedKey::ArrowDown) => {
                menu.move_selection(1);
                None
            }
            Key::Named(NamedKey::Enter) => menu.selected().and_then(|v| v.as_bool()),
            Key::Character(c) if c.as_str() == "1" => Some(true),
            Key::Character(c) if c.as_str() == "2" => Some(false),
            _ => None,
        };
        let Some(install) = choice else { return };
        self.overlay = Overlay::None;
        let Some(m) = self.update_offer.clone() else { return };
        if !install {
            self.update_offer = None;
            return;
        }
        if self.tabs.iter().any(|t| t.doc.is_dirty() && t.doc.path().is_some()) {
            self.show("Save your files first, then: Help: Check for Updates");
            return;
        }
        self.show(format!("Downloading Wyven {} …", m.version));
        let source = self.settings.update.url.trim().to_string();
        let proxy = self.proxy.clone();
        self.update_rx = Some(update::download(source, m, move || {
            let _ = proxy.send_event(UserEvent::Update);
        }));
    }

    /// Every command of the search field: name for keymap.toml, title, default keys, command.
    fn all_commands(&self) -> Vec<(String, String, String, palette::Cmd)> {
        let mut out: Vec<(String, String, String, palette::Cmd)> = palette::commands()
            .into_iter()
            .map(|(title, keys, cmd)| (custom_keys::command_id(title), title.to_string(), keys.to_string(), cmd))
            .collect();
        for (title, _, cmd) in &self.palette.plugin_commands {
            out.push((custom_keys::command_id(title), title.clone(), String::new(), cmd.clone()));
        }
        out
    }

    fn command_by_id(&self, id: &str) -> Option<palette::Cmd> {
        self.all_commands().into_iter().find(|c| c.0 == id).map(|c| c.3)
    }

    fn run_palette_cmd(&mut self, cmd: palette::Cmd) {
        match cmd {
            palette::Cmd::Key(c) => self.run(c),
            palette::Cmd::Leader(a) => self.leader_action(a),
            palette::Cmd::Plugin(i, id) => self.run_plugin_command(i, &id),
        }
        self.window.request_redraw();
    }

    /// Reads keymap.toml (at start and after saving it in Wyven).
    fn reload_keymap(&mut self, report: bool) {
        let ids: Vec<String> = self.all_commands().into_iter().map(|c| c.0).collect();
        let (keys, errors) = custom_keys::CustomKeys::load(|id| ids.iter().any(|i| i == id));
        self.custom_keys = keys;
        if let Some(e) = errors.last() {
            self.show(e.clone());
        } else if report {
            self.show(if self.custom_keys.is_empty() { "keymap.toml: no own shortcuts" } else { "Own shortcuts applied" });
        }
    }

    /// Loads the plugins folder (again) and puts their commands into the search field.
    fn reload_plugins(&mut self, report: bool) {
        let errors = self.plugins.load_all();
        self.refresh_plugin_commands();
        if let Some(e) = errors.last() {
            self.show(e.clone());
        } else if report {
            self.show(format!("{} plugin(s) loaded", self.plugins.len()));
        }
    }

    fn refresh_plugin_commands(&mut self) {
        self.palette.plugin_commands = self
            .plugins
            .commands()
            .into_iter()
            .map(|(i, id, title)| (title, "plugin".to_string(), palette::Cmd::Plugin(i, id)))
            .collect();
    }

    /// The active file as plugins see it.
    fn plugin_editor(&self) -> plugins::Editor {
        let ed = self.editor();
        let r = ed.doc.selection().primary();
        plugins::Editor {
            path: ed.doc.path().map(|p| p.display().to_string()),
            language: ed.language_name().to_string(),
            text: ed.doc.text().to_string(),
            selection_start: r.from(),
            selection_end: r.to(),
        }
    }

    fn run_plugin_command(&mut self, index: usize, id: &str) {
        if self.editor().settings_page || self.diff.is_some() {
            self.show("Plugin commands work on files.");
            return;
        }
        let editor = self.plugin_editor();
        match self.plugins.run_command(index, id, editor) {
            Ok(actions) => self.apply_plugin_actions(actions),
            Err(e) => {
                self.show(e);
                self.refresh_plugin_commands();
            }
        }
    }

    /// Tells the plugins that want it about an event in the active tab.
    fn plugin_event(&mut self, event: plugins::Event) {
        if !self.plugins.wants(event) || self.editor().settings_page {
            return;
        }
        let editor = self.plugin_editor();
        let (actions, errors) = self.plugins.event(event, &editor);
        self.apply_plugin_actions(actions);
        if let Some(e) = errors.last() {
            self.show(e.clone());
            self.refresh_plugin_commands();
        }
    }

    /// Carries out what a plugin answered (each edit is one undo step).
    fn apply_plugin_actions(&mut self, actions: Vec<plugins::Action>) {
        use plugins::Action as A;
        let mut edited = false;
        for action in actions {
            let ed = &mut self.tabs[self.active];
            let len = ed.doc.text().len_chars();
            match action {
                A::ReplaceAll { text } => {
                    if ed.doc.text() != text.as_str() {
                        let head = ed.doc.selection().primary().head;
                        ed.doc.replace_range(0, len, &text);
                        let new_len = ed.doc.text().len_chars();
                        ed.doc.set_cursor(head.min(new_len));
                        edited = true;
                    }
                }
                A::ReplaceSelection { text } => {
                    let r = ed.doc.selection().primary();
                    ed.doc.replace_range(r.from().min(len), r.to().min(len), &text);
                    edited = true;
                }
                A::ReplaceRange { start, end, text } => {
                    let end = end.min(len);
                    ed.doc.replace_range(start.min(end), end, &text);
                    edited = true;
                }
                A::SetSelection { start, end } => {
                    let (s, e) = (start.min(len), end.min(len));
                    ed.doc.set_selection(wyven_buffer::Selection::single(wyven_buffer::Range::new(s, e)));
                }
                A::Message { text } => self.show(text),
            }
        }
        let ed = &mut self.tabs[self.active];
        if edited {
            ed.after_edit();
        }
        ed.ensure_cursor_visible();
        if edited {
            self.lsp_after_edit();
        }
        self.window.request_redraw();
    }

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

    /// Re-reads settings.toml and applies what changed. A broken file keeps the old settings.
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
        let new_theme = new.theme != self.settings.theme;
        self.settings = new;
        if new_theme {
            match Theme::resolve(&self.settings.theme) {
                Ok(theme) => self.apply_theme(theme),
                Err(e) => self.show(e),
            }
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
    /// Switches to a theme and stores it in settings.toml.
    fn set_theme(&mut self, name: &str) {
        let theme = match Theme::resolve(name) {
            Ok(t) => t,
            Err(e) => {
                self.show(e);
                return;
            }
        };
        let display = theme.display_name();
        self.apply_theme(theme);
        self.settings.theme = name.to_string();
        if let Err(e) = settings::store_theme(name) {
            self.show(format!("Theme not saved: {e}"));
        } else {
            self.settings_mtime = settings::modified();
            self.show(format!("Theme: {display}"));
        }
    }

    /// Shows a theme everywhere (without storing it: also the picker's preview).
    fn apply_theme(&mut self, theme: Theme) {
        self.terminal.dirty = true;
        let fs = self.renderer.font_system();
        self.find_labels.set(fs, &self.family, &theme);
        self.theme = theme;
        for t in &mut self.tabs {
            t.invalidate();
        }
        if let Some(sp) = &mut self.split {
            for t in &mut sp.tabs {
                t.invalidate();
            }
        }
        if let Some(ex) = &mut self.explorer {
            ex.invalidate();
        }
        if let Some(d) = &mut self.diff {
            d.invalidate();
        }
        self.window.request_redraw();
    }

    /// Alt → v → c: all themes; moving through the list previews them.
    fn open_theme_picker(&mut self) {
        let mut all: Vec<(String, String)> =
            theme::THEMES.iter().map(|(id, label)| (label.to_string(), id.to_string())).collect();
        all.extend(vscode_theme::discover().into_iter().map(|e| (format!("{}   · {}", e.label, e.source), e.label)));
        let before = self.settings.theme.clone();
        self.theme_picker = Some(ThemePicker { all, filter: String::new(), before });
        self.refill_theme_picker();
    }

    /// Builds the list for the current filter, with the current theme selected.
    fn refill_theme_picker(&mut self) {
        let Some(p) = &self.theme_picker else { return };
        let f = p.filter.to_lowercase();
        let items: Vec<(String, wyven_lsp::Value)> = p
            .all
            .iter()
            .filter(|(label, _)| f.is_empty() || label.to_lowercase().contains(&f))
            .map(|(label, id)| (label.clone(), wyven_lsp::Value::String(id.clone())))
            .collect();
        let title = if p.filter.is_empty() {
            "Color theme (type to filter · ↑↓ preview · Enter keeps · Esc back)".to_string()
        } else {
            format!("Color theme: {}▏", p.filter)
        };
        let current = self.theme.name;
        let at = items.iter().position(|(_, v)| {
            v.as_str().is_some_and(|id| Theme::by_name(id).map_or(id == current, |t| t.name == current))
        });
        let fs = self.renderer.font_system();
        let mut menu = menu::Menu::new(fs, self.ui_font, &title, items);
        if let Some(i) = at {
            menu.select(i);
        }
        self.overlay = Overlay::Themes(menu);
    }

    fn preview_selected_theme(&mut self) {
        let Overlay::Themes(menu) = &self.overlay else { return };
        let Some(id) = menu.selected().and_then(|v| v.as_str().map(str::to_string)) else { return };
        if Theme::by_name(&id).map_or(id.as_str(), |t| t.name) == self.theme.name {
            return;
        }
        match Theme::resolve(&id) {
            Ok(t) => self.apply_theme(t),
            Err(e) => self.show(e),
        }
    }

    fn theme_picker_key(&mut self, event: &KeyEvent, key: &Key) {
        let Overlay::Themes(menu) = &mut self.overlay else { return };
        match key {
            Key::Named(NamedKey::Escape) => {
                self.overlay = Overlay::None;
                if let Some(p) = self.theme_picker.take()
                    && let Ok(t) = Theme::resolve(&p.before)
                {
                    self.apply_theme(t);
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                menu.move_selection(-1);
                self.preview_selected_theme();
            }
            Key::Named(NamedKey::ArrowDown) => {
                menu.move_selection(1);
                self.preview_selected_theme();
            }
            Key::Named(NamedKey::PageUp) => {
                menu.move_selection(-10);
                self.preview_selected_theme();
            }
            Key::Named(NamedKey::PageDown) => {
                menu.move_selection(10);
                self.preview_selected_theme();
            }
            Key::Named(NamedKey::Enter) => {
                let id = menu.selected().and_then(|v| v.as_str().map(str::to_string));
                self.overlay = Overlay::None;
                self.theme_picker = None;
                if let Some(id) = id {
                    self.set_theme(&id);
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(p) = &mut self.theme_picker {
                    p.filter.pop();
                }
                self.refill_theme_picker();
                self.preview_selected_theme();
            }
            _ => {
                let typed = event.text.as_ref().map(|t| t.to_string()).unwrap_or_default();
                let typed: String = typed.chars().filter(|c| !c.is_control()).collect();
                if !typed.is_empty() && !self.modifiers.control_key() {
                    if let Some(p) = &mut self.theme_picker {
                        p.filter.push_str(&typed);
                    }
                    self.refill_theme_picker();
                    self.preview_selected_theme();
                }
            }
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
            .active_root()
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
        let roots = self.roots();
        if roots.is_empty() {
            self.show("No folder open: Alt → p → o opens one.");
            return;
        }
        let prefill = find::FindBar::prefill_from(&self.editor().doc);
        self.search.show(&roots, replace, prefill);
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
            .active_root()
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
                if let Some(path) = index::resolve(&self.roots(), &rel) {
                    self.open_path(&path);
                }
            }
            A::Run(Cmd::Key(cmd)) => self.run(cmd),
            A::Run(Cmd::Leader(action)) => self.leader_action(action),
            A::Run(Cmd::Plugin(index, id)) => self.run_plugin_command(index, &id),
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
        if group == "w"
            && let Key::Named(arrow @ (NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::ArrowUp | NamedKey::ArrowDown)) = key
        {
            self.focus_half(matches!(arrow, NamedKey::ArrowLeft | NamedKey::ArrowUp));
            return;
        }
        let c = match key {
            Key::Named(NamedKey::Escape) => return,
            Key::Character(s) => s.chars().next(),
            _ => None,
        };
        let Some(c) = c else { return };
        if let Some(id) = self.custom_keys.leader(group, c).map(str::to_string)
            && let Some(cmd) = self.command_by_id(&id)
        {
            self.run_palette_cmd(cmd);
            return;
        }
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
            Split { vertical } => self.split_editor(vertical),
            FocusOtherHalf => self.focus_other_half(),
            CloseSplit => self.close_split(),
            ToggleZen => self.toggle_zen(),
            ToggleWordWrap => self.toggle_word_wrap(),
            OpenFolder => self.pick_folder(),
            ExplorerOp(op) => self.start_explorer_op(op),
            AddFolder => self.add_folder(),
            OpenWorkspace => self.open_workspace_dialog(),
            SaveWorkspace => self.save_workspace_as(),
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
            OpenSettings => self.open_settings_page(),
            ReloadPlugins => self.reload_plugins(true),
            CheckUpdates => self.check_updates(true),
            ShowPlugins => {
                let list = self.plugins.summary();
                let dir = plugins::dir().map(|d| d.display().to_string()).unwrap_or_default();
                if list.is_empty() {
                    self.show(format!("No plugins installed. Put .wasm files into {dir}"));
                } else {
                    self.show(format!("Plugins: {}", list.join("  ·  ")));
                }
            }
            OpenPluginsFolder => {
                if let Some(dir) = plugins::dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(e) = std::process::Command::new("explorer.exe").arg(&dir).spawn() {
                        self.show(format!("{}: {e}", dir.display()));
                    }
                }
            }
            OpenKeymap => {
                let Some(path) = custom_keys::path() else { return };
                if !path.exists() {
                    let list: Vec<(String, String, String)> =
                        self.all_commands().into_iter().map(|(id, title, keys, _)| (id, title, keys)).collect();
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    if let Err(e) = std::fs::write(&path, custom_keys::template(&list)) {
                        self.show(format!("keymap.toml: {e}"));
                        return;
                    }
                }
                self.open_path(&path);
            }
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
            ChooseTheme => self.open_theme_picker(),
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

    /// Alt → v → w: word wrap on/off for all tabs, saved in settings.toml.
    fn toggle_word_wrap(&mut self) {
        let on = !self.settings.editor.word_wrap;
        self.settings.editor.word_wrap = on;
        for t in &mut self.tabs {
            t.wrap = on;
            t.scroll_x = 0.0;
            t.invalidate();
            t.ensure_cursor_visible();
        }
        let label = if on { "Word wrap: on" } else { "Word wrap: off" };
        match settings::store_value(Some("editor"), "word_wrap", if on { "true" } else { "false" }) {
            Ok(()) => {
                self.settings_mtime = settings::modified();
                self.show(label);
            }
            Err(e) => self.show(format!("{label} (not saved: {e})")),
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
    fn request_implementation(&mut self, pos: usize) {
        if self.tabs[self.active].lsp_kind().is_none() {
            self.show("Go to implementation needs a language server (C#, TypeScript).");
            return;
        }
        if !self.lsp_ready() {
            self.show("The language server is still loading the project …");
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
            server.implementation(path, ed.doc.text(), pos);
        }
    }

    /// Alt → c → o. C#: remove unused usings (step 1), then sort (step 2); other languages:
    /// their "organize imports" action.
    fn request_organize(&mut self, step: u8) {
        self.organize_step = None;
        if self.tabs[self.active].lsp_kind().is_none() {
            self.show("Organizing usings needs a language server (C#, TypeScript).");
            return;
        }
        if !self.lsp_ready() {
            self.show("The language server is still loading the project …");
            return;
        }
        self.lsp_sync_active();
        let ed = &self.tabs[self.active];
        let rope = ed.doc.text();
        let range = if ed.lsp_kind() == Some(lsp::ServerKind::CSharp) {
            match csharp_usings(rope) {
                Some(r) => r,
                None => {
                    self.show("No usings in this file");
                    return;
                }
            }
        } else {
            (0, rope.len_chars())
        };
        self.organize_step = Some(step);
        let ed = &self.tabs[self.active];
        if let (Some(server), Some(path)) = (server_of(&mut self.servers, ed), ed.doc.path()) {
            server.organize_actions(path, ed.doc.text(), range.0, range.1);
        }
    }

    fn organize_actions(&mut self, actions: Vec<wyven_lsp::Value>) {
        let Some(step) = self.organize_step.take() else { return };
        let title = |a: &wyven_lsp::Value| a["title"].as_str().unwrap_or("").to_string();
        let kind = |a: &wyven_lsp::Value| a["kind"].as_str().unwrap_or("").to_string();
        if let Some(a) = actions.iter().find(|a| kind(a).starts_with("source.organizeImports")) {
            self.run_action(a.clone());
            return;
        }
        let remove = actions.iter().find(|a| title(a) == "Remove unnecessary usings");
        let sort = actions.iter().find(|a| title(a) == "Sort Usings");
        match (step, remove, sort) {
            (1, Some(r), _) => {
                self.organize_step = Some(2);
                self.run_action(r.clone());
            }
            (_, _, Some(s)) => self.run_action(s.clone()),
            (1, None, None) => self.show("The usings are already tidy"),
            _ => self.show("Removed unnecessary usings"),
        }
    }

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
        self.show_locations(refs, "reference");
    }

    /// A pick list of places ("reference", "implementation").
    fn show_locations(&mut self, refs: Vec<(PathBuf, wyven_lsp::Value)>, noun: &str) {
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
        let title = format!("{} {noun}{} (digit or Enter, Esc)", refs.len(), if refs.len() == 1 { "" } else { "s" });
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
            Command::GotoImplementation => {
                follow_cursor = false;
                let pos = self.editor().doc.selection().primary().head;
                self.request_implementation(pos);
            }
            Command::OrganizeUsings => {
                follow_cursor = false;
                self.request_organize(1);
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
        if let Overlay::Context(menu) = &self.overlay {
            let action = (button == MouseButton::Left).then(|| menu.row_at(self.mouse.x, self.mouse.y)).flatten().and_then(|r| menu.action(r));
            let inside = menu.contains(self.mouse.x, self.mouse.y);
            self.overlay = Overlay::None;
            if let Some(a) = action {
                self.run_ctx(a);
            }
            self.window.request_redraw();
            if inside || button != MouseButton::Right {
                return;
            }
            // A right-click elsewhere opens the menu there (below).
        } else if !matches!(self.overlay, Overlay::None) {
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
        if button == MouseButton::Left
            && let Some(item) = self.activity.hit(x, y)
        {
            self.activity_click(item);
            return;
        }
        if button == MouseButton::Left {
            if let Some(area) = self.splitter_at(x, y) {
                self.resizing = Some(area);
                return;
            }
            match self.dock_bar.hit(x, y) {
                Some(dock::HeaderHit::Tab(tab)) => {
                    self.panel_drag = Some(PanelDrag { what: Dragged::Panel(tab.panel), from: (x, y), moved: false, targets: Vec::new(), target: None });
                    self.activate_panel(tab.panel);
                    return;
                }
                Some(dock::HeaderHit::Button(_, panel, button)) => {
                    self.header_button(panel, button);
                    return;
                }
                None => {}
            }
        }
        let inside = |r: Option<Rect>| r.is_some_and(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h);
        if inside(self.layout.tabs2) || inside(self.layout.editor2) {
            // The other half: it gets the focus, then the click works as usual.
            self.focus_other_half();
        }
        let t = self.layout.tabs;
        if y >= t.y && y < t.y + t.h && x >= t.x && x < t.x + t.w {
            if let Some(i) = self.tab_spans.iter().position(|&(a, b)| x >= a && x < b) {
                let (cx0, cx1) = tab_close_span(self.tab_spans[i], self.ui_font.cell_w);
                match button {
                    MouseButton::Middle => self.close_tab(i),
                    MouseButton::Left if x >= cx0 && x < cx1 => self.close_tab(i),
                    MouseButton::Right => self.open_tab_menu(i, x, y),
                    MouseButton::Left => {
                        self.activate(i);
                        self.focus = Focus::Editor;
                        // Dragging a tab shows the compass to split the editor.
                        self.panel_drag = Some(PanelDrag { what: Dragged::Tab(i), from: (x, y), moved: false, targets: Vec::new(), target: None });
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
        if explorer_shown
            && button == MouseButton::Right
            && self.explorer.as_ref().is_some_and(|ex| ex.contains(x, y))
        {
            self.open_explorer_menu(x, y);
            return;
        }
        if explorer_shown
            && button == MouseButton::Left
            && let Some(root) = self.explorer.as_ref().and_then(|ex| ex.close_hit(x, y))
        {
            self.focus = Focus::Explorer;
            self.remove_folder(&root);
            return;
        }
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
        if button == MouseButton::Right && self.diff.is_none() && self.editor().contains(x, y) && !self.editor().settings_page {
            self.open_editor_menu(x, y);
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
        if let (Some(sp), Some(e2)) = (&mut self.split, self.layout.editor2)
            && x >= e2.x
            && x < e2.x + e2.w
            && y >= e2.y
            && y < e2.y + e2.h
        {
            sp.tabs[sp.active].scroll_by(-dy.round() as isize);
            sp.tabs[sp.active].invalidate();
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
        let project = match (&self.workspace_file, self.project_root()) {
            (Some(f), _) => format!(" – {}", f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            (None, Some(r)) => format!(" – {}", file_name(r)),
            (None, None) => String::new(),
        };
        let title = format!("{dirty}{}{project} – Wyven", ed.file_name());
        if title != self.title {
            self.window.set_title(&title);
            self.title = title;
        }
    }

    fn update_tab_bar(&mut self) {
        let cw = self.ui_font.cell_w;
        let (spans, tab_spans) = tab_bar_spans(&self.tabs, self.active, self.layout.tabs.x, cw, &self.theme);
        self.tab_spans = tab_spans;
        let attrs = Attrs::new().family(Family::Name(&self.family));
        if let (Some(sp), Some(t2)) = (&mut self.split, self.layout.tabs2) {
            let (labels, spans2) = tab_bar_spans(&sp.tabs, sp.active, t2.x, cw, &self.theme);
            sp.tab_spans = spans2;
            sp.tab_bar.set_rich_text(
                labels.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
                &attrs,
                Shaping::Advanced,
                None,
            );
            sp.tab_bar.shape_until_scroll(self.renderer.font_system(), false);
        }
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
        let dark = self.theme.dark;
        let (title, color, entries): (String, Rgba, Vec<popup::PopupEntry>) = match &self.overlay {
            Overlay::Leader { group, shown: true, .. } => (
                leader::title(group),
                chrome::group_color(group, dark),
                leader::items(group)
                    .iter()
                    .map(|i| popup::PopupEntry {
                        key: i.key.to_string(),
                        label: i.label.to_string(),
                        group: matches!(i.action, leader::Action::Group(_)),
                    })
                    .collect(),
            ),
            Overlay::Recent(list) => (
                "Recent projects (pick a digit, Esc = cancel)".into(),
                self.theme.accent,
                list.iter()
                    .enumerate()
                    .map(|(i, p)| popup::PopupEntry { key: (i + 1).to_string(), label: p.display().to_string(), group: false })
                    .collect(),
            ),
            _ => return false,
        };
        let fs = self.renderer.font_system();
        self.popup.set(fs, &self.family, &title, color, &entries);
        true
    }

    fn redraw(&mut self) {
        self.sync_split_twins();
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
            Overlay::Branches(m)
            | Overlay::DebugProjects(m)
            | Overlay::Processes(m)
            | Overlay::References(m)
            | Overlay::Themes(m)
            | Overlay::Update(m)
            | Overlay::ConfirmDelete(m, _) => {
                m.prepare(self.renderer.font_system(), &self.theme, &self.family)
            }
            Overlay::ExplorerPrompt { prompt, .. } => prompt.prepare(self.renderer.font_system(), &self.theme, &self.family),
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
            if let (Some(sp), Some(_)) = (&mut self.split, self.layout.editor2) {
                sp.tabs[sp.active].prepare(self.renderer.font_system(), theme);
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
        if self.zen {
            let (w, h) = self.renderer.size();
            under.push(Quad { x: 0.0, y: 0.0, w: w as f32, h: h as f32, color: theme.background });
        }
        let mut over = Vec::with_capacity(16);
        let mut overlay = Vec::new();
        let mut text: Vec<TextBlock<'_>> = Vec::with_capacity(8);
        let mut overlay_text: Vec<TextBlock<'_>> = Vec::new();
        let mut cards: Vec<RoundRect> = Vec::new();
        let mut masks: Vec<RoundRect> = Vec::new();
        let mut overlay_cards: Vec<RoundRect> = Vec::new();
        // The cards everything sits on, rounded; the focused one gets a colored edge.
        if !self.zen {
            let rad = l.radius;
            let focus_color = match focused_panel {
                Some(p) => chrome::panel_color(p, theme.dark),
                None => theme.accent,
            };
            let focused_card = match focused_panel {
                Some(p) => self.layout_area(self.dock.area_of(p)),
                None => Some(l.editor_card),
            };
            let mut all: Vec<(Rect, Rgba)> = vec![(l.editor_card, theme.background)];
            if let Some(c) = l.editor2_card {
                all.push((c, theme.background));
            }
            for r in [l.left, l.right, l.bottom].into_iter().flatten() {
                all.push((r, theme.panel_bg));
            }
            for (r, color) in all {
                cards.push(RoundRect::new(r.x, r.y, r.w, r.h, color, rad));
                masks.push(RoundRect::corner_mask(r.x, r.y, r.w, r.h, theme.frame, rad));
                if focused_card == Some(r) {
                    let edge = (1.5 * scale).max(1.0);
                    masks.push(RoundRect::outline(r.x, r.y, r.w, r.h, focus_color.with_alpha(0xB0), rad, edge));
                } else {
                    masks.push(RoundRect::outline(r.x, r.y, r.w, r.h, theme.divider, rad, scale.round().max(1.0)));
                }
            }
            let shown_panels: Vec<Panel> = Panel::ALL.into_iter().filter(|p| self.panel_shown(*p)).collect();
            self.activity.draw(theme, scale, &shown_panels, focused_panel, &mut cards, &mut text);
        }

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
        self.dock_bar.draw(theme, scale, &strips, focused_panel, &mut under, &mut cards, &mut text);

        // Tab bar: the active tab lifted, a colored line under it, × on the active and the
        // hovered tab (● for unsaved changes).
        under.push(Quad { x: l.tabs.x, y: l.tabs.y, w: l.tabs.w, h: l.tabs.h, color: theme.tab_bar });
        let accent_h = (2.0 * scale).round();
        let tab_clip = [l.tabs.x as i32, l.tabs.y as i32, (l.tabs.x + l.tabs.w) as i32, (l.tabs.y + l.tabs.h) as i32];
        let cw = self.ui_font.cell_w;
        let icon_h = (self.ui_font.size * 0.8).round() * 1.3;
        for (i, &(x0, x1)) in self.tab_spans.iter().enumerate() {
            let hovered = self.tab_hover == Some(i);
            if i == self.active {
                under.push(Quad { x: x0, y: l.tabs.y, w: x1 - x0, h: l.tabs.h, color: theme.tab_active });
                let line = if focused_panel.is_none() { theme.accent } else { theme.accent.with_alpha(0x70) };
                cards.push(RoundRect::new(x0 + cw, l.tabs.y + l.tabs.h - accent_h - 1.0, x1 - x0 - 2.0 * cw, accent_h, line, accent_h / 2.0));
            } else if hovered {
                under.push(Quad { x: x0, y: l.tabs.y, w: x1 - x0, h: l.tabs.h, color: theme.tab_active.with_alpha(0x80) });
            }
            under.push(Quad { x: x1, y: l.tabs.y + l.tabs.h * 0.25, w: 1.0, h: l.tabs.h * 0.5, color: theme.divider });
            let dirty = self.tabs.get(i).is_some_and(|t| t.doc.is_dirty());
            let (cx0, cx1) = tab_close_span((x0, x1), cw);
            let over_close = hovered && self.mouse.x >= cx0 && self.mouse.x < cx1;
            let icon = if dirty && !hovered {
                Some(&self.tab_icons.1)
            } else if hovered || i == self.active {
                Some(&self.tab_icons.0)
            } else {
                None
            };
            if let Some(icon) = icon {
                if over_close {
                    let side = cx1 - cx0;
                    cards.push(RoundRect::new(cx0, l.tabs.y + (l.tabs.h - side) / 2.0, side, side, theme.tab_text.with_alpha(0x30), (4.0 * scale).round()));
                }
                let iw = icon.layout_runs().next().map_or(cw, |r| r.line_w);
                text.push(TextBlock {
                    buffer: icon,
                    x: (cx0 + (cx1 - cx0 - iw) / 2.0).round(),
                    y: (l.tabs.y + (l.tabs.h - icon_h) / 2.0).round(),
                    color: if dirty && !hovered { theme.tab_text_active } else { theme.tab_text },
                    clip: Some(tab_clip),
                });
            }
        }
        text.push(TextBlock {
            buffer: &self.tab_bar,
            x: l.tabs.x,
            y: l.tabs.y + (l.tabs.h - self.ui_font.line_h) / 2.0,
            color: theme.tab_text,
            clip: Some(tab_clip),
        });

        match &self.diff {
            Some(d) => d.draw(theme, scale, &mut under, &mut over, &mut text),
            None if self.tabs[self.active].settings_page => self.settings_page.draw(theme, &mut under, &mut text),
            None => self.tabs[self.active].draw(theme, scale, &mut under, &mut over, &mut text),
        }
        if let (Some(sp), Some(t2), Some(e2)) = (&self.split, l.tabs2, l.editor2) {
            under.push(Quad { x: t2.x, y: t2.y, w: t2.w, h: t2.h, color: theme.tab_bar });
            for (i, &(x0, x1)) in sp.tab_spans.iter().enumerate() {
                if i == sp.active {
                    under.push(Quad { x: x0, y: t2.y, w: x1 - x0, h: t2.h, color: theme.tab_active });
                }
                under.push(Quad { x: x1, y: t2.y, w: 1.0, h: t2.h, color: theme.divider });
            }
            text.push(TextBlock {
                buffer: &sp.tab_bar,
                x: t2.x,
                y: t2.y + (t2.h - self.ui_font.line_h) / 2.0,
                color: theme.tab_text,
                clip: Some([t2.x as i32, t2.y as i32, (t2.x + t2.w) as i32, (t2.y + t2.h) as i32]),
            });
            sp.tabs[sp.active].draw(theme, scale, &mut under, &mut over, &mut text);
            // The line between the halves.
            let e1 = l.editor;
            let (dx, dy, dw, dh) = if sp.vertical {
                (e1.x.max(e2.x) - 1.0, t2.y.min(l.tabs.y), 1.0, e2.h + t2.h)
            } else {
                (e2.x, e1.y.max(e2.y) - l.tabs.h - 1.0, e2.w, 1.0)
            };
            under.push(Quad { x: dx, y: dy, w: dw, h: dh, color: theme.divider });
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
            clip: Some([0, s.y as i32, s.w as i32, (s.y + s.h) as i32]),
        });
        // The AI switch, left of the right-hand status text (click toggles).
        let ai_w = self.status_ai.layout_runs().next().map_or(0.0, |r| r.line_w);
        let ai_x = s.w - right_w - 3.0 * pad - ai_w;
        self.status_ai_span = (ai_x - pad * 0.5, ai_x + ai_w + pad * 0.5);
        let ai_color = if self.ai.enabled && self.ai.has_key() { theme.status_accent } else { theme.status_text };
        let status_clip = Some([0, s.y as i32, s.w as i32, (s.y + s.h) as i32]);
        text.push(TextBlock { buffer: &self.status_ai, x: ai_x, y: text_y, color: ai_color, clip: status_clip });

        if popup_visible {
            self.popup.draw(theme, scale, s.w, s.y, &mut overlay_cards, &mut overlay, &mut overlay_text);
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
                Overlay::Branches(m)
                | Overlay::DebugProjects(m)
                | Overlay::Processes(m)
                | Overlay::References(m)
                | Overlay::Themes(m)
                | Overlay::Update(m)
                | Overlay::ConfirmDelete(m, _) => {
                    let (x, y) = (l.editor.x + self.ui_font.cell_w * 4.0, l.editor.y + self.ui_font.line_h);
                    m.draw(theme, self.ui_font, x, y, window, scale, &mut overlay, &mut overlay_text)
                }
                Overlay::ExplorerPrompt { prompt, .. } => {
                    let (x, y) = (l.editor.x + self.ui_font.cell_w * 4.0, l.editor.y + self.ui_font.line_h);
                    prompt.draw(theme, self.ui_font, x, y, window, scale, &mut overlay, &mut overlay_text)
                }
                Overlay::NewBranch(prompt) | Overlay::Watch(prompt) | Overlay::ApiKey(prompt) => {
                    let (x, y) = (l.editor.x + self.ui_font.cell_w * 4.0, l.editor.y + self.ui_font.line_h);
                    prompt.draw(theme, self.ui_font, x, y, window, scale, &mut overlay, &mut overlay_text)
                }
                _ => {}
            }
        }
        if let Overlay::Context(menu) = &self.overlay {
            menu.draw(theme, scale, &mut overlay_cards, &mut overlay, &mut overlay_text);
        }
        // The docking compass while a panel or tab is dragged.
        if let Some(d) = self.panel_drag.as_ref().filter(|d| d.moved) {
            let color = match d.what {
                Dragged::Panel(p) => chrome::panel_color(p, theme.dark),
                Dragged::Tab(_) => theme.accent,
            };
            compass::draw(&d.targets, d.target, color, theme, scale, &mut overlay_cards);
        }
        if matches!(self.overlay, Overlay::Palette) {
            let top = l.tabs.y + l.tabs.h * 0.5;
            self.palette.draw(theme, self.ui_font, s.w, top, scale, &mut overlay, &mut overlay_text);
        }

        let scene = Scene {
            background: if self.zen { theme.background } else { theme.frame },
            cards: &cards,
            under: &under,
            text: &text,
            over: &over,
            masks: &masks,
            overlay_cards: &overlay_cards,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_its_own_keymap_name() {
        let mut seen = std::collections::HashMap::new();
        for (title, _, _) in palette::commands() {
            let id = custom_keys::command_id(title);
            assert!(!id.is_empty(), "{title}");
            if let Some(other) = seen.insert(id.clone(), title) {
                panic!("“{title}” and “{other}” both become {id}");
            }
        }
    }

    #[test]
    fn file_names_are_checked_like_windows_does() {
        assert!(valid_file_name("Program.cs").is_ok());
        assert!(valid_file_name("a/b").is_err());
        assert!(valid_file_name("what?").is_err());
        assert!(valid_file_name("con.txt").is_err());
        assert!(valid_file_name("name.").is_err());
        assert!(valid_file_name("").is_err());
    }

    #[test]
    fn the_using_block_is_found_at_the_top() {
        let rope = wyven_buffer::Rope::from_str("// header\nusing System.Text;\n\nusing System;\nnamespace A;\nusing (var x = y) { }\n");
        let (start, end) = csharp_usings(&rope).unwrap();
        assert_eq!(rope.slice(start..end).to_string(), "using System.Text;\n\nusing System;\n");
        assert_eq!(csharp_usings(&wyven_buffer::Rope::from_str("class A {}\n")), None);
        let only = wyven_buffer::Rope::from_str("global using System;");
        assert_eq!(csharp_usings(&only), Some((0, only.len_chars())));
    }
}
