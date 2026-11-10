// Release builds on Windows open no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Wyvern – Phase 0: a window that draws text on the GPU and measures RAM and input latency.
//!
//! The text area here is a placeholder to test input -> render. The real text buffer
//! (rope, undo, multi-cursor) comes in Phase 1.

mod metrics;

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};
use wyvern_render::{
    Attrs, Buffer, ClearColor, Color, Family, Metrics, Renderer, Shaping, TextBlock,
};

use metrics::InputLatency;

const FONT_SIZE: f32 = 15.0;
const LINE_HEIGHT: f32 = 22.0;
const PADDING: f32 = 16.0;
const STATS_INTERVAL: Duration = Duration::from_millis(500);

/// #16181D, converted to linear space for the sRGB surface.
const BACKGROUND: ClearColor = ClearColor { r: 0.0080, g: 0.0091, b: 0.0123, a: 1.0 };
const TEXT: Color = Color::rgb(0xD4, 0xD7, 0xDE);
const DIM: Color = Color::rgb(0x7A, 0x80, 0x8C);

const WELCOME: &str = "Wyvern – Phase 0\n\nTippe hier, um die Eingabe-Latenz zu messen.\nBackspace löscht, Enter macht eine neue Zeile.\n\n";

/// Startup profiling: logs elapsed time and RAM after each startup stage.
fn log_stage(stage: &str, started: Instant) {
    let ram = metrics::ram_mb().map_or("–".into(), |mb| format!("{mb:.0} MB"));
    log::info!("startup {stage}: {} · RAM {ram}", metrics::ms(Some(started.elapsed())));
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    renderer: Renderer,
    scale: f32,

    text: String,
    text_buffer: Buffer,
    status_buffer: Buffer,

    latency: InputLatency,
    started: Instant,
    first_frame: Option<Duration>,
    next_stats: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let started = Instant::now();
        let attrs = Window::default_attributes()
            .with_title("Wyvern")
            .with_inner_size(LogicalSize::new(1100.0, 720.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        log_stage("window", started);
        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            event_loop.owned_display_handle(),
        ));
        log_stage("renderer", started);
        self.state = Some(State::new(window, renderer, started));
        log_stage("state", started);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                state.renderer.resize(size.width, size.height);
                state.layout();
                state.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.scale = scale_factor as f32;
                state.layout();
                state.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => state.key(event),
            WindowEvent::RedrawRequested => state.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Wake up twice a second to refresh the RAM/latency line, otherwise sleep.
        let Some(state) = &mut self.state else { return };
        if Instant::now() >= state.next_stats {
            state.next_stats = Instant::now() + STATS_INTERVAL;
            state.update_status();
            state.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(state.next_stats));
    }
}

impl State {
    fn new(window: Arc<Window>, mut renderer: Renderer, started: Instant) -> Self {
        let scale = window.scale_factor() as f32;
        let metrics = Metrics::new(FONT_SIZE * scale, LINE_HEIGHT * scale);
        let text_buffer = Buffer::new(renderer.font_system(), metrics);
        let status_buffer = Buffer::new(renderer.font_system(), metrics);
        let mut state = Self {
            window,
            renderer,
            scale,
            text: WELCOME.to_string(),
            text_buffer,
            status_buffer,
            latency: InputLatency::default(),
            started,
            first_frame: None,
            next_stats: Instant::now(),
        };
        state.layout();
        state.window.request_redraw();
        state
    }

    fn metrics(&self) -> Metrics {
        Metrics::new(FONT_SIZE * self.scale, LINE_HEIGHT * self.scale)
    }

    /// Re-applies size and font metrics (after resize or DPI change) and re-shapes all text.
    fn layout(&mut self) {
        let (w, h) = self.renderer.size();
        let metrics = self.metrics();
        let pad = PADDING * self.scale;
        self.text_buffer.set_metrics(metrics);
        self.text_buffer
            .set_size(Some(w as f32 - 2.0 * pad), Some(h as f32 - 3.0 * pad));
        self.status_buffer.set_metrics(metrics);
        self.status_buffer.set_size(Some(w as f32 - 2.0 * pad), None);
        self.set_text();
        self.update_status();
    }

    fn set_text(&mut self) {
        let mut shown = self.text.clone();
        shown.push('▏'); // placeholder cursor
        self.text_buffer.set_text(
            &shown,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.text_buffer
            .shape_until_scroll(self.renderer.font_system(), false);
    }

    fn update_status(&mut self) {
        let gpu = self.renderer.gpu_info().clone();
        let ram = metrics::ram_mb().map_or("–".into(), |mb| format!("{mb:.0} MB"));
        let status = format!(
            "RAM {ram}  ·  Taste→Bild {} (Ø {}, max {})  ·  Start {}  ·  {} ({}, {})",
            metrics::ms(self.latency.last()),
            metrics::ms(self.latency.average()),
            metrics::ms(self.latency.max()),
            metrics::ms(self.first_frame),
            gpu.name,
            gpu.backend,
            gpu.present_mode,
        );
        self.status_buffer.set_text(
            &status,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.status_buffer
            .shape_until_scroll(self.renderer.font_system(), false);
    }

    fn key(&mut self, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let changed = match &event.logical_key {
            Key::Named(NamedKey::Backspace) => self.text.pop().is_some(),
            Key::Named(NamedKey::Enter) => {
                self.text.push('\n');
                true
            }
            Key::Named(NamedKey::Tab) => {
                self.text.push_str("    ");
                true
            }
            _ => match &event.text {
                Some(t) if !t.chars().any(char::is_control) => {
                    self.text.push_str(t);
                    true
                }
                _ => false,
            },
        };
        if changed {
            self.latency.input();
            self.set_text();
            self.window.request_redraw();
        }
    }

    fn redraw(&mut self) {
        let (_, h) = self.renderer.size();
        let pad = PADDING * self.scale;
        let status_y = h as f32 - pad - LINE_HEIGHT * self.scale;
        let blocks = [
            TextBlock { buffer: &self.text_buffer, x: pad, y: pad, color: TEXT },
            TextBlock { buffer: &self.status_buffer, x: pad, y: status_y, color: DIM },
        ];
        if self.renderer.draw(BACKGROUND, &blocks) {
            self.latency.presented();
            if self.first_frame.is_none() {
                self.first_frame = Some(self.started.elapsed());
                log_stage("first frame", self.started);
            }
        } else {
            self.window.request_redraw();
        }
    }
}
