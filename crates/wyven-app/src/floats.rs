//! Floating panels, as in Visual Studio: a panel dragged out of the window and released where
//! no docking target is becomes a window of its own. Its tab strip can be dragged back over the
//! main window, where the docking compass appears again. Closing the window closes the panel
//! (it comes back where it was docked before). Position and size are saved with the layout.
//!
//! The windows draw with the main renderer (same GPU device, fonts and glyph atlas).

use std::sync::Arc;

use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};
use wyven_render::{Quad, RoundRect, Scene, TextBlock};

use crate::dock::{self, Panel, Place};
use crate::editor::Rect;
use crate::{Dragged, Focus, PanelDrag, State};

/// An OS window showing a floating pane.
pub struct FloatWin {
    pub window: Arc<Window>,
    bar: dock::DockBar,
    /// The panels of its floating entry (to find the entry again after changes).
    panels: Vec<Panel>,
    mouse: (f32, f32),
}

fn title(panels: &[Panel]) -> String {
    let names: Vec<&str> = panels.iter().map(|p| p.name()).collect();
    format!("{} – Wyven", names.join(", "))
}

impl State {
    /// Opens and closes OS windows so there is one for every floating pane.
    pub(crate) fn sync_floats(&mut self, event_loop: &ActiveEventLoop) {
        let same = self.floats.len() == self.dock.floating.len()
            && self.floats.iter().zip(&self.dock.floating).all(|(w, f)| w.panels == f.panels);
        if same {
            return;
        }
        let wanted: Vec<dock::Floating> = self.dock.floating.clone();
        let mut old = std::mem::take(&mut self.floats);
        let mut new = Vec::new();
        for f in &wanted {
            if let Some(i) = old.iter().position(|w| w.panels.iter().any(|p| f.panels.contains(p))) {
                let mut w = old.remove(i);
                if w.panels != f.panels {
                    w.window.set_title(&title(&f.panels));
                    w.panels = f.panels.clone();
                }
                w.window.request_redraw();
                new.push(w);
            } else if let Some(w) = self.make_float(event_loop, f) {
                new.push(w);
            }
        }
        for w in old {
            self.renderer.remove_window(w.window.id());
        }
        self.floats = new;
        self.relayout();
    }

    fn make_float(&mut self, event_loop: &ActiveEventLoop, f: &dock::Floating) -> Option<FloatWin> {
        let [x, y, w, h] = f.rect;
        let icon = winit::window::Icon::from_rgba(include_bytes!("../../../assets/icon-64.rgba").to_vec(), 64, 64).ok();
        let attrs = Window::default_attributes()
            .with_title(title(&f.panels))
            .with_window_icon(icon)
            .with_inner_size(PhysicalSize::new(w.max(160) as u32, h.max(120) as u32))
            .with_position(PhysicalPosition::new(x, y));
        let window = Arc::new(event_loop.create_window(attrs).ok()?);
        if !self.renderer.add_window(window.clone()) {
            log::warn!("cannot draw into a floating window");
            return None;
        }
        let bar = dock::DockBar::new(self.renderer.font_system(), self.ui_font, self.family.clone());
        window.request_redraw();
        Some(FloatWin { window, bar, panels: f.panels.clone(), mouse: (0.0, 0.0) })
    }

    pub(crate) fn is_float_window(&self, id: WindowId) -> bool {
        self.floats.iter().any(|f| f.window.id() == id)
    }

    /// The floating entry shown in window `wi`.
    fn float_entry(&self, wi: usize) -> Option<usize> {
        let w = self.floats.get(wi)?;
        self.dock.floating.iter().position(|f| f.panels.iter().any(|p| w.panels.contains(p)))
    }

    fn float_active(&self, fi: usize) -> Panel {
        let f = &self.dock.floating[fi];
        f.panels[f.active.min(f.panels.len() - 1)]
    }

    /// Asks every floating window to draw again (after the main window changed).
    pub(crate) fn redraw_floats(&self) {
        for f in &self.floats {
            f.window.request_redraw();
        }
    }

    /// Where a point of floating window `wi` is in main window coordinates.
    fn to_main(&self, wi: usize, x: f32, y: f32) -> (f32, f32) {
        let pos = |w: &Window| w.inner_position().map(|p| (p.x as f32, p.y as f32)).unwrap_or((0.0, 0.0));
        let (fx, fy) = pos(&self.floats[wi].window);
        let (mx, my) = pos(&self.window);
        (x + fx - mx, y + fy - my)
    }

    /// A screen point (physical pixels) from main window coordinates.
    pub(crate) fn main_to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        let (mx, my) = self.window.inner_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));
        (mx + x as i32, my + y as i32)
    }

    pub(crate) fn float_event(&mut self, id: WindowId, event: WindowEvent) {
        let Some(wi) = self.floats.iter().position(|f| f.window.id() == id) else { return };
        match event {
            WindowEvent::CloseRequested => {
                if let Some(fi) = self.float_entry(wi) {
                    for p in self.dock.floating[fi].panels.clone() {
                        self.dock.hide(p);
                    }
                    if self.focused_panel().is_some_and(|p| !self.dock.shown(p)) {
                        self.focus = Focus::Editor;
                    }
                    self.dock_changed();
                }
            }
            WindowEvent::Resized(size) => {
                self.renderer.resize_window(id, size.width, size.height);
                if let Some(fi) = self.float_entry(wi) {
                    let r = &mut self.dock.floating[fi].rect;
                    r[2] = size.width as i32;
                    r[3] = size.height as i32;
                    self.save_layout_soon();
                }
                self.floats[wi].window.request_redraw();
            }
            WindowEvent::Moved(_) => {
                if let (Some(fi), Ok(p)) = (self.float_entry(wi), self.floats[wi].window.inner_position()) {
                    let r = &mut self.dock.floating[fi].rect;
                    r[0] = p.x;
                    r[1] = p.y;
                    self.save_layout_soon();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => self.floats[wi].window.request_redraw(),
            WindowEvent::Focused(true) => {
                if let Some(fi) = self.float_entry(wi) {
                    self.focus_panel(self.float_active(fi));
                    self.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => self.key_event(&event),
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x as f32, position.y as f32);
                self.floats[wi].mouse = (x, y);
                if self.panel_drag.is_some() {
                    let (mx, my) = self.to_main(wi, x, y);
                    self.panel_drag_move(mx, my);
                    self.window.request_redraw();
                }
                if self.dock.neighbours(Panel::Terminal).iter().any(|p| self.floats[wi].panels.contains(p)) {
                    let h = self.terminal.list_hit(x, y).map(|h| h.0);
                    if h != self.terminal.list_hover {
                        self.terminal.list_hover = h;
                        self.floats[wi].window.request_redraw();
                    }
                }
                let hover = match self.floats[wi].bar.hit(x, y) {
                    Some(dock::HeaderHit::Button(a, _, b)) => Some((a, b)),
                    _ => None,
                };
                if hover != self.floats[wi].bar.hover {
                    self.floats[wi].bar.hover = hover;
                    self.floats[wi].window.request_redraw();
                }
                if self.terminal.selecting {
                    let (row, col) = self.terminal.cell_at(x, y);
                    if let Some(t) = self.terminal.term() {
                        t.select_to(row, col);
                    }
                    self.terminal.dirty = true;
                    self.floats[wi].window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed {
                    self.float_press(wi, button);
                } else {
                    self.mouse_button(button, false);
                }
                self.redraw_floats();
                self.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.float_wheel(wi, delta);
                self.floats[wi].window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw_float(wi),
            _ => {}
        }
    }

    /// Moves the keyboard focus to a panel.
    fn focus_panel(&mut self, p: Panel) {
        self.focus = match p {
            Panel::Explorer => Focus::Explorer,
            Panel::Search => Focus::Search,
            Panel::Git => Focus::Git,
            Panel::Debug => Focus::Debug,
            Panel::Problems => Focus::Problems,
            Panel::Terminal => Focus::Terminal,
        };
    }

    fn float_press(&mut self, wi: usize, button: MouseButton) {
        let (x, y) = self.floats[wi].mouse;
        let Some(fi) = self.float_entry(wi) else { return };
        let active = self.float_active(fi);
        if button == MouseButton::Left {
            match self.floats[wi].bar.hit(x, y) {
                Some(dock::HeaderHit::Tab(t)) => {
                    self.dock.show(t.panel);
                    self.focus_panel(t.panel);
                    let from = self.to_main(wi, x, y);
                    self.panel_drag =
                        Some(PanelDrag { what: Dragged::Panel(t.panel), from, at: from, moved: false, targets: Vec::new(), target: None });
                    self.dock.save();
                    return;
                }
                Some(dock::HeaderHit::Button(_, p, b)) => {
                    self.header_button(p, b);
                    return;
                }
                None => {}
            }
        }
        self.focus_panel(active);
        let left = button == MouseButton::Left;
        match active {
            Panel::Terminal if self.terminal.list_hit(x, y).is_some() => {
                if let Some((i, close)) = self.terminal.list_hit(x, y) {
                    match (button, close) {
                        (MouseButton::Left, false) => self.terminal.select(i),
                        (MouseButton::Left, true) | (MouseButton::Middle, _) => self.close_terminal(i),
                        _ => {}
                    }
                }
            }
            Panel::Terminal if self.terminal.contains(x, y) && left => {
                let (row, col) = self.terminal.cell_at(x, y);
                if let Some(t) = self.terminal.term() {
                    t.select_start(row, col);
                }
                self.terminal.selecting = true;
                self.terminal.dirty = true;
            }
            Panel::Search if self.search.contains(x, y) && left => {
                let action = self.search.click(y);
                self.apply_panel_action(action);
            }
            Panel::Git if self.git.contains(x, y) && left => {
                let action = self.git.click_at(x, y, false);
                self.apply_git_action(action);
            }
            Panel::Problems if self.problems.contains(x, y) && left => {
                let action = self.problems.click(y);
                self.apply_problem_action(action);
            }
            Panel::Debug if self.debugger.contains(x, y) && left => {
                let action = self.debugger.click(y);
                self.apply_debug_panel_action(action);
            }
            Panel::Explorer if left => {
                if let Some(root) = self.explorer.as_ref().and_then(|ex| ex.close_hit(x, y)) {
                    self.remove_folder(&root);
                    return;
                }
                if let Some(ex) = &mut self.explorer
                    && ex.contains(x, y)
                    && let Some(i) = ex.row_at(y)
                    && let Some(path) = ex.activate(i)
                {
                    self.open_path(&path);
                    self.focus = Focus::Editor;
                    self.window.focus_window();
                }
            }
            _ => {}
        }
    }

    fn float_wheel(&mut self, wi: usize, delta: MouseScrollDelta) {
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * 3.0,
            MouseScrollDelta::PixelDelta(p) => (p.y / self.font.line_h as f64) as f32,
        };
        let Some(fi) = self.float_entry(wi) else { return };
        let step = -dy.round() as isize;
        match self.float_active(fi) {
            Panel::Terminal => {
                if let Some(t) = self.terminal.term() {
                    t.scroll(dy.round() as i32);
                }
                self.terminal.dirty = true;
            }
            Panel::Search => self.search.scroll_by(step),
            Panel::Git => self.git.scroll_by(step),
            Panel::Debug => self.debugger.scroll_by(step),
            Panel::Problems => self.problems.scroll_by(step),
            Panel::Explorer => {
                if let Some(ex) = &mut self.explorer {
                    ex.scroll_by(step);
                }
            }
        }
    }

    fn redraw_float(&mut self, wi: usize) {
        let Some(fi) = self.float_entry(wi) else { return };
        let id = self.floats[wi].window.id();
        let Some((w, h)) = self.renderer.window_size(id) else { return };
        let f = self.dock.floating[fi].clone();
        let active = self.float_active(fi);
        let rect = Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        let strip = dock::Strip { rect, panels: f.panels.clone(), active, title_only: false };
        self.floats[wi].bar.place(std::slice::from_ref(&strip));
        let c = dock::content(rect, self.ui_font);
        if active == Panel::Terminal && !self.terminal_started {
            self.terminal.set_area(c);
            self.start_terminal();
        }
        // The panel's place and its text for this frame.
        let focused = self.focused_panel() == Some(active);
        let active_path = self.editor().doc.path().map(|p| p.to_path_buf());
        match active {
            Panel::Explorer => {
                if let Some(ex) = &mut self.explorer {
                    ex.set_area(c);
                    ex.prepare(self.renderer.font_system(), &self.theme, active_path.as_deref());
                }
            }
            Panel::Search => {
                self.search.set_area(c);
                self.search.prepare(self.renderer.font_system(), &self.theme, focused);
            }
            Panel::Git => {
                self.git.set_area(c);
                self.git.prepare(self.renderer.font_system(), &self.theme, focused);
            }
            Panel::Debug => {
                self.debugger.set_area(c);
                let bps = self.all_breakpoints();
                self.debugger.prepare(self.renderer.font_system(), &self.theme, focused, &bps);
            }
            Panel::Terminal => {
                self.terminal.set_area(c);
                self.terminal.prepare(self.renderer.font_system(), &self.theme, focused);
            }
            Panel::Problems => {
                self.problems.set_area(c);
                self.problems.prepare(self.renderer.font_system(), &self.theme, focused);
            }
        }
        let theme = &self.theme;
        let scale = self.renderer.scale_factor();
        let mut under: Vec<Quad> = vec![Quad { x: 0.0, y: 0.0, w: rect.w, h: rect.h, color: theme.panel_bg }];
        let mut over: Vec<Quad> = Vec::new();
        let mut text: Vec<TextBlock<'_>> = Vec::new();
        let mut cards: Vec<RoundRect> = Vec::new();
        let focused_panel = if focused { Some(active) } else { None };
        match active {
            Panel::Explorer => {
                if let Some(ex) = &self.explorer {
                    ex.draw(theme, focused, &mut under, &mut text);
                }
            }
            Panel::Search => self.search.draw(theme, focused, &mut under, &mut text),
            Panel::Git => self.git.draw(theme, focused, &mut under, &mut text),
            Panel::Debug => self.debugger.draw(theme, focused, &mut under, &mut text),
            Panel::Terminal => self.terminal.draw(theme, focused, scale, &mut under, &mut over, &mut text),
            Panel::Problems => self.problems.draw(theme, focused, &mut under, &mut text),
        }
        self.floats[wi].bar.draw(theme, scale, std::slice::from_ref(&strip), focused_panel, &mut under, &mut cards, &mut text);
        // A colored line at the top: the panel's color, bright while it has the focus.
        let color = crate::chrome::panel_color(active, theme.dark);
        over.push(Quad { x: 0.0, y: 0.0, w: rect.w, h: (2.0 * scale).round(), color: if focused { color } else { color.with_alpha(0x60) } });
        let scene = Scene {
            background: theme.panel_bg,
            cards: &cards,
            under: &under,
            text: &text,
            over: &over,
            masks: &[],
            overlay_cards: &[],
            overlay: &[],
            overlay_text: &[],
        };
        if !self.renderer.draw_window(id, &scene) {
            self.floats[wi].window.request_redraw();
        }
    }

    /// A panel released where no target is: it floats at the mouse (`at` in main window
    /// coordinates), as large as it was docked.
    pub(crate) fn float_panel(&mut self, p: Panel, at: (f32, f32)) {
        // Alone in its floating window already: the window just stays.
        if matches!(self.dock.place(p), Place::Floating(i) if self.dock.floating[i].panels.len() == 1) {
            return;
        }
        let size = self.pane_rect(p).map_or((420.0, 520.0), |r| (r.w.max(260.0), r.h.clamp(200.0, 700.0)));
        let (sx, sy) = self.main_to_screen(at.0, at.1);
        let grab = (self.ui_font.cell_w * 4.0) as i32;
        self.dock.float(p, [sx - grab, sy - (self.ui_font.line_h * 0.8) as i32, size.0 as i32, size.1 as i32]);
        self.focus_panel(p);
        self.dock_changed();
    }
}
