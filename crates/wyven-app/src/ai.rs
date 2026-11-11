//! AI suggestions while typing: when to ask the model (local or Claude), and which answers
//! still fit.
//!
//! After every typed character the request is scheduled for when typing pauses
//! (`[ai] delay_ms`). Answers arrive on another thread; one is shown only if the same tab,
//! text version and cursor position are still current.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use wyven_ai::{Backend, Context, Event, Failure, Generation, Model};

/// The backend from `[ai]` in settings.toml: `provider = "local"` (default) or `"claude"`.
pub fn backend_from(settings: &crate::settings::AiSettings) -> Backend {
    if settings.provider.trim().eq_ignore_ascii_case("claude") {
        Backend::Claude(Model::parse(&settings.model))
    } else {
        let m = settings.local_model.trim();
        Backend::Local(if m.is_empty() { wyven_ai::DEFAULT_LOCAL_MODEL.to_string() } else { m.to_string() })
    }
}

/// What the app should do after AI events.
pub enum AiUpdate {
    /// Show (or update, while streaming) the suggestion at `pos` in tab `tab`.
    Show { tab: usize, version: u64, pos: usize, text: String },
    Message(String),
}

struct Pending {
    id: u64,
    tab: usize,
    version: u64,
    pos: usize,
    /// Text after the cursor (a few lines; for trimming what the model repeats).
    after: String,
}

pub struct AiAssist {
    pub enabled: bool,
    pub backend: Backend,
    key: Option<String>,
    /// The key was rejected (401/403): no more requests until a new one is set.
    key_rejected: bool,
    generation: Generation,
    pending: Option<Pending>,
    pub due: Option<Instant>,
    delay: Duration,
    tx: Sender<(u64, Event)>,
    rx: Receiver<(u64, Event)>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    paused_until: Option<Instant>,
    last_error: Option<Instant>,
    /// The last keystroke: suggestions appear only `show_after` later.
    last_typed: Option<Instant>,
    show_after: Duration,
    /// A finished suggestion waiting for the pause: tab, version, position, text.
    pub held: Option<(usize, u64, usize, String)>,
}

impl AiAssist {
    pub fn new(enabled: bool, backend: Backend, delay_ms: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        if let (true, Backend::Local(m)) = (enabled, &backend) {
            wyven_ai::warm_up(m.clone());
        }
        Self {
            enabled,
            backend,
            key: wyven_ai::api_key(),
            key_rejected: false,
            generation: Generation::default(),
            pending: None,
            due: None,
            delay: Duration::from_millis(delay_ms.clamp(100, 5000)),
            tx,
            rx,
            wake: None,
            paused_until: None,
            last_error: None,
            last_typed: None,
            show_after: Duration::from_millis(4000),
            held: None,
        }
    }

    /// How long typing must pause before a (already computed) suggestion appears.
    pub fn set_show_after(&mut self, ms: u64) {
        self.show_after = Duration::from_millis(ms.min(30_000));
    }

    /// When the held suggestion may appear.
    pub fn show_due(&self) -> Option<Instant> {
        self.held.as_ref()?;
        Some(self.last_typed.map_or_else(Instant::now, |t| t + self.show_after))
    }

    /// The held suggestion, once the pause is long enough.
    pub fn take_due(&mut self, now: Instant) -> Option<(usize, u64, usize, String)> {
        if self.show_due().is_some_and(|due| now >= due) { self.held.take() } else { None }
    }

    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(Arc::new(wake));
    }

    pub fn configure(&mut self, enabled: bool, backend: Backend, delay_ms: u64) {
        if backend != self.backend || (enabled && !self.enabled) {
            self.paused_until = None;
            if let (true, Backend::Local(m)) = (enabled, &backend) {
                wyven_ai::warm_up(m.clone());
            }
        }
        self.enabled = enabled;
        self.backend = backend;
        self.delay = Duration::from_millis(delay_ms.clamp(100, 5000));
        if !enabled {
            self.cancel();
        }
    }

    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// Reads the key again (after it was set or removed).
    pub fn reload_key(&mut self) {
        self.key = wyven_ai::api_key();
        self.key_rejected = false;
    }

    /// The current backend can run (Claude needs a key that was not rejected).
    pub fn usable(&self) -> bool {
        !self.backend.needs_key() || (self.key.is_some() && !self.key_rejected)
    }

    /// Status bar label.
    pub fn label(&self) -> String {
        if self.backend.needs_key() && self.key.is_none() {
            "✦ AI: no key".into()
        } else if self.backend.needs_key() && self.key_rejected {
            "✦ AI: key rejected".into()
        } else if !self.enabled {
            "✦ AI off".into()
        } else {
            format!("✦ {}", self.backend.display())
        }
    }

    fn ready(&self, now: Instant) -> bool {
        self.enabled && self.usable() && self.paused_until.is_none_or(|t| now >= t)
    }

    /// Typing happened: ask again once typing pauses.
    pub fn schedule(&mut self, now: Instant) {
        self.cancel();
        self.last_typed = Some(now);
        if self.ready(now) {
            self.due = Some(now + self.delay);
        }
    }

    /// Anything else happened (cursor moved, Esc): forget the pending request.
    pub fn cancel(&mut self) {
        self.due = None;
        self.held = None;
        if self.pending.take().is_some() {
            self.generation.next();
        }
    }

    pub fn start(&mut self, tab: usize, version: u64, pos: usize, ctx: Context) {
        self.due = None;
        if !self.usable() {
            return;
        }
        let id = self.generation.next();
        let after: String = ctx.after.split_inclusive('\n').take(30).collect();
        self.pending = Some(Pending { id, tab, version, pos, after });
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        let key = self.key.clone();
        wyven_ai::request(self.backend.clone(), key, ctx, self.generation.clone(), id, move |id, ev| {
            if tx.send((id, ev)).is_ok()
                && let Some(w) = &wake
            {
                w();
            }
        });
    }

    pub fn poll(&mut self, now: Instant) -> Vec<AiUpdate> {
        let mut out = Vec::new();
        while let Ok((id, ev)) = self.rx.try_recv() {
            let Some(p) = self.pending.as_ref().filter(|p| p.id == id) else { continue };
            match ev {
                Event::Partial(text) | Event::Done(text) => {
                    let text = wyven_ai::clean_suggestion(&text, &p.after);
                    out.push(AiUpdate::Show { tab: p.tab, version: p.version, pos: p.pos, text });
                }
                Event::Failed(f) => {
                    self.pending = None;
                    match f {
                        Failure::BadKey => {
                            self.key_rejected = true;
                            out.push(AiUpdate::Message(
                                "Claude rejected the API key – Alt → a → k sets a new one".into(),
                            ));
                        }
                        Failure::NoLocalServer => {
                            self.paused_until = Some(now + Duration::from_secs(10));
                            out.push(AiUpdate::Message(
                                "No connection to Ollama – start it from the Start menu (AI suggestions retry in 10 s)".into(),
                            ));
                        }
                        Failure::ModelMissing(m) => {
                            self.paused_until = Some(now + Duration::from_secs(60));
                            out.push(AiUpdate::Message(format!(
                                "Model {m} is missing – in a terminal: ollama pull {m}"
                            )));
                        }
                        Failure::Busy => {
                            self.paused_until = Some(now + Duration::from_secs(30));
                            out.push(AiUpdate::Message("Claude is busy – AI suggestions pause for 30 s".into()));
                        }
                        Failure::Other(m) => {
                            // Not every few seconds while offline.
                            if self.last_error.is_none_or(|t| now.duration_since(t) > Duration::from_secs(60)) {
                                self.last_error = Some(now);
                                out.push(AiUpdate::Message(format!("AI suggestion failed: {m}")));
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

/// Whether a suggestion makes sense here: nothing but closing brackets, quotes, `;` or `,`
/// may follow the cursor on its line.
pub fn fits_cursor(rest_of_line: &str) -> bool {
    rest_of_line.chars().all(|c| c.is_whitespace() || ")]}>;,\"'`".contains(c))
}

/// The next piece of a suggestion for Ctrl+→: leading spaces plus one word, or one other
/// character.
pub fn next_word(text: &str) -> &str {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut end = 0;
    let mut chars = text.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if c == ' ' || c == '\t' {
            end = i + c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }
    match chars.next() {
        Some((i, c)) if word(c) => {
            end = i + c.len_utf8();
            for (j, c) in chars {
                if !word(c) {
                    break;
                }
                end = j + c.len_utf8();
            }
        }
        Some((i, c)) => end = i + c.len_utf8(),
        None => {}
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_taken_one_by_one() {
        assert_eq!(next_word("Console.WriteLine(x);"), "Console");
        assert_eq!(next_word(".WriteLine(x);"), ".");
        assert_eq!(next_word("  foo bar"), "  foo");
        assert_eq!(next_word("\n    next"), "\n");
        assert_eq!(next_word(""), "");
    }

    #[test]
    fn suggestions_only_where_they_fit() {
        assert!(fits_cursor(""));
        assert!(fits_cursor(");"));
        assert!(!fits_cursor(" foo);"));
    }
}
