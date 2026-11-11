//! Inline code suggestions ("ghost text").
//!
//! Two backends, one request per suggestion, streamed, on a background thread:
//! - **Local** (default, free): a code model in Ollama on this PC (`/api/generate` with
//!   `prompt` = text before the cursor and `suffix` = text after it, fill-in-the-middle).
//! - **Claude**: the Anthropic Messages API. Rust has no official Anthropic SDK, so this speaks
//!   raw HTTP (ureq) and parses the SSE stream itself. The API key lives in the Windows
//!   Credential Manager (keyring), with the `ANTHROPIC_API_KEY` environment variable as a
//!   fallback.
//!
//! Prompt layout for prompt caching: the start of the file (whole 50-line chunks well above
//! the cursor) is one cached block; typing near the cursor does not change it, so repeated
//! suggestions in a large file mostly pay the cheap cache-read rate.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
const KEYRING_SERVICE: &str = "Wyven";
const KEYRING_USER: &str = "anthropic-api-key";

/// At most this many lines per suggestion.
pub const MAX_LINES: usize = 10;
/// Context limits (characters) around the cursor.
const MAX_BEFORE: usize = 60_000;
const MAX_AFTER: usize = 6_000;
/// The cached block covers whole chunks of this many lines, ending this far above the cursor.
const CACHE_CHUNK_LINES: usize = 50;
const CACHE_GAP_LINES: usize = 30;

const SYSTEM_PROMPT: &str = "You are the code completion engine of a code editor. The user message \
contains a file; <CURSOR/> marks where the user's cursor is. Reply with exactly the text to insert \
at the cursor and nothing else: no explanation, no markdown fences, and no repetition of code that \
is already before or after the cursor. Continue the current line; when the next lines are obvious \
(for example the body of a block the user just opened), complete them too, at most 10 lines. Match \
the file's indentation, naming and style. If there is nothing useful to add, reply with an empty \
message.";

/// Where Ollama listens (only on this PC).
const OLLAMA_URL: &str = "http://127.0.0.1:11434/api/generate";
/// The default local model: the *base* variant, trained for fill-in-the-middle. The instruct
/// variant (`qwen2.5-coder:7b`) answers like a chat, with explanations and markdown.
pub const DEFAULT_LOCAL_MODEL: &str = "qwen2.5-coder:7b-base";
/// Qwen2.5-Coder's fill-in-the-middle markers and the tokens that end a fill.
const FIM_PREFIX: &str = "<|fim_prefix|>";
const FIM_SUFFIX: &str = "<|fim_suffix|>";
const FIM_MIDDLE: &str = "<|fim_middle|>";
const FIM_STOP: &[&str] = &[
    "<|endoftext|>",
    "<|fim_prefix|>",
    "<|fim_suffix|>",
    "<|fim_middle|>",
    "<|fim_pad|>",
    "<|file_sep|>",
    "<|repo_name|>",
    "<|im_start|>",
    "<|im_end|>",
    "```",
];
/// Local models get less context: they are slower per token and have a small window.
const LOCAL_BEFORE: usize = 6_000;
const LOCAL_AFTER: usize = 1_500;

/// Which model answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// An Ollama model on this PC, e.g. "qwen2.5-coder:7b".
    Local(String),
    Claude(Model),
}

impl Backend {
    pub fn display(&self) -> String {
        match self {
            Self::Local(m) => {
                // "qwen2.5-coder:7b" -> "Qwen2.5-Coder 7B (local)"
                let (name, size) = m.split_once(':').unwrap_or((m, ""));
                let size = size.trim_end_matches("-base");
                let name: String = name
                    .split('-')
                    .map(|w| {
                        let mut c = w.chars();
                        c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
                    })
                    .collect::<Vec<_>>()
                    .join("-");
                let size = if size.is_empty() { String::new() } else { format!(" {}", size.to_uppercase()) };
                format!("{name}{size} (local)")
            }
            Self::Claude(m) => m.display().to_string(),
        }
    }

    pub fn needs_key(&self) -> bool {
        matches!(self, Self::Claude(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// Fast and cheap: the default for suggestions while typing.
    Haiku,
    /// Better suggestions, a bit slower.
    Sonnet,
}

impl Model {
    pub fn parse(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("sonnet") { Self::Sonnet } else { Self::Haiku }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Haiku => "haiku",
            Self::Sonnet => "sonnet",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Self::Haiku => "Haiku 5.5",
            Self::Sonnet => "Sonnet 5.5",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Haiku => "claude-haiku-5-5",
            Self::Sonnet => "claude-sonnet-5-5",
        }
    }
}

// ----- API key -----

/// The stored API key: Windows Credential Manager first, then `ANTHROPIC_API_KEY`.
pub fn api_key() -> Option<String> {
    let stored = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .and_then(|e| e.get_password())
        .ok()
        .filter(|k| !k.trim().is_empty());
    stored.or_else(|| std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty()))
}

/// Stores the key in the Windows Credential Manager; an empty key removes it.
pub fn store_api_key(key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| e.to_string())?;
    let key = key.trim();
    if key.is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    entry.set_password(key).map_err(|e| e.to_string())
}

// ----- prompt -----

/// What the editor sends: the file text split at the cursor.
pub struct Context {
    pub file_name: String,
    pub language: String,
    pub before: String,
    pub after: String,
}

/// Splits `before` into a stable, cacheable head (whole chunks of lines well above the cursor)
/// and the rest.
fn split_for_cache(before: &str) -> (&str, &str) {
    let line_starts: Vec<usize> =
        std::iter::once(0).chain(before.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let cursor_line = line_starts.len().saturating_sub(1);
    let head_lines = cursor_line.saturating_sub(CACHE_GAP_LINES) / CACHE_CHUNK_LINES * CACHE_CHUNK_LINES;
    if head_lines == 0 {
        return ("", before);
    }
    before.split_at(line_starts[head_lines])
}

/// Cuts the context to the limits, keeping the parts closest to the cursor.
fn clip(ctx: &Context) -> (String, String) {
    clip_to(ctx, MAX_BEFORE, MAX_AFTER)
}

fn clip_to(ctx: &Context, max_before: usize, max_after: usize) -> (String, String) {
    let before = &ctx.before;
    let before = if before.len() > max_before {
        let mut start = before.len() - max_before;
        while !before.is_char_boundary(start) {
            start += 1;
        }
        // Start at a line boundary so the cached head stays stable while typing.
        let start = before[start..].find('\n').map_or(start, |i| start + i + 1);
        &before[start..]
    } else {
        before
    };
    let after = &ctx.after;
    let after = if after.len() > max_after {
        let mut end = max_after;
        while !after.is_char_boundary(end) {
            end -= 1;
        }
        &after[..end]
    } else {
        after
    };
    (before.to_string(), after.to_string())
}

/// The Ollama request: fill in the middle between the text before and after the cursor. The
/// prompt is sent raw with Qwen's own FIM markers, so no chat template gets in the way.
pub fn local_body(model: &str, ctx: &Context) -> Value {
    let (before, after) = clip_to(ctx, LOCAL_BEFORE, LOCAL_AFTER);
    json!({
        "model": model,
        "prompt": format!("{FIM_PREFIX}{before}{FIM_SUFFIX}{after}{FIM_MIDDLE}"),
        "raw": true,
        "stream": true,
        // Keep the model in GPU memory between suggestions.
        "keep_alive": "30m",
        "options": {
            "num_predict": 200,
            "temperature": 0.2,
            "num_ctx": 4096,
            "stop": FIM_STOP,
        },
    })
}

/// The request body (also used by the tests).
pub fn request_body(model: Model, ctx: &Context) -> Value {
    let (before, after) = clip(ctx);
    let (head, tail) = split_for_cache(&before);
    let opening = format!("<file name=\"{}\" language=\"{}\">\n", ctx.file_name, ctx.language);
    let mut content = Vec::new();
    let rest = if head.is_empty() {
        format!("{opening}{tail}<CURSOR/>{after}\n</file>")
    } else {
        content.push(json!({
            "type": "text",
            "text": format!("{opening}{head}"),
            "cache_control": { "type": "ephemeral" },
        }));
        format!("{tail}<CURSOR/>{after}\n</file>")
    };
    content.push(json!({ "type": "text", "text": rest }));
    let mut body = json!({
        "model": model.id(),
        "max_tokens": 1024,
        "stream": true,
        "system": [{ "type": "text", "text": SYSTEM_PROMPT }],
        "output_config": { "effort": "low" },
        "messages": [{ "role": "user", "content": content }],
    });
    match model {
        // No thinking: suggestions must come fast. Allowed on Haiku 5.5 at effort high or below.
        Model::Haiku => body["thinking"] = json!({ "type": "disabled" }),
        // Sonnet 5.5 rejects disabled thinking; adaptive at low effort skips it on most
        // simple requests. Server-side fallbacks keep a suggestion coming if a safety
        // classifier declines the request.
        Model::Sonnet => body["fallbacks"] = json!("default"),
    }
    body
}

// ----- response cleanup -----

/// Removes what the model should not have sent: markdown fences, an overlap with the text
/// after the cursor, and lines beyond the limit.
pub fn clean_suggestion(raw: &str, after: &str) -> String {
    let mut s = raw.to_string();
    if let Some(rest) = s.strip_prefix("```") {
        // Drop the opening fence line (with its language tag) and a closing fence.
        s = rest.split_once('\n').map_or(String::new(), |(_, r)| r.to_string());
        if let Some(i) = s.rfind("```") {
            s.truncate(i);
        }
    }
    // A chat-style model writes code, then a closing fence and an explanation: keep the code.
    if let Some(i) = s.find("```") {
        s.truncate(i);
    }
    // Special tokens must never end up in the code.
    if let Some(i) = s.find("<|") {
        s.truncate(i);
    }
    let s = drop_repeated_lines(&s.replace("<CURSOR/>", ""), after);
    // At most MAX_LINES lines.
    let mut out: String = s.split_inclusive('\n').take(MAX_LINES).collect();
    if out.ends_with('\n') && s.split_inclusive('\n').count() > MAX_LINES {
        out.pop();
    }
    // The model sometimes repeats what already follows the cursor on this line (")" or ";").
    let rest_of_line = after.split('\n').next().unwrap_or("");
    let trimmed_end = out.trim_end_matches([' ', '\t', '\n', '\r']).len();
    out.truncate(trimmed_end);
    for len in (1..=rest_of_line.len().min(out.len())).rev() {
        if rest_of_line.is_char_boundary(len) && out.ends_with(&rest_of_line[..len]) {
            out.truncate(out.len() - len);
            break;
        }
    }
    out
}

/// Removes trailing lines of a suggestion that repeat the lines right after the cursor's line
/// (models like to close blocks that are already closed).
fn drop_repeated_lines(s: &str, after: &str) -> String {
    let lines: Vec<&str> = s.trim_end().split('\n').collect();
    let next: Vec<&str> = after.split('\n').skip(1).filter(|l| !l.trim().is_empty()).collect();
    let max = lines.len().saturating_sub(1).min(next.len());
    for k in (1..=max).rev() {
        let tail = &lines[lines.len() - k..];
        if tail.iter().zip(&next[..k]).all(|(a, b)| a.trim() == b.trim()) {
            return lines[..lines.len() - k].join("\n");
        }
    }
    s.to_string()
}

// ----- request -----

/// Updates from a running request.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The suggestion so far (grows while streaming).
    Partial(String),
    Done(String),
    Failed(Failure),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// 401/403: the key is wrong.
    BadKey,
    /// 429 or 529: back off for a while.
    Busy,
    /// Ollama does not answer (not installed or not running).
    NoLocalServer,
    /// Ollama answers but does not have this model.
    ModelMissing(String),
    Other(String),
}

/// Bumped for every new request; a running request stops as soon as it is outdated.
#[derive(Clone, Default)]
pub struct Generation(Arc<AtomicU64>);

impl Generation {
    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Starts a suggestion request on a background thread. `sink` receives the events of request
/// `id`; once `generation` moves past `id` the request stops quietly.
pub fn request(
    backend: Backend,
    key: Option<String>,
    ctx: Context,
    generation: Generation,
    id: u64,
    sink: impl Fn(u64, Event) + Send + 'static,
) {
    std::thread::Builder::new()
        .name("ai".into())
        .spawn(move || {
            let ev = match &backend {
                Backend::Local(model) => run_local(model, &ctx, &generation, id, &sink),
                Backend::Claude(model) => match &key {
                    Some(key) => run(key, *model, &ctx, &generation, id, &sink),
                    None => Some(Event::Failed(Failure::BadKey)),
                },
            };
            if let Some(ev) = ev {
                sink(id, ev);
            }
        })
        .ok();
}

/// Loads a local model into (GPU) memory in the background, so the first suggestion does not
/// wait for it (loading takes up to a minute; afterwards suggestions take well under a second).
/// Errors are ignored: a missing server or model is reported by the first real request.
pub fn warm_up(model: String) {
    std::thread::Builder::new()
        .name("ai-warm-up".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(180)))
                .build()
                .into();
            let body = json!({ "model": model, "keep_alive": "30m" }).to_string();
            match agent.post(OLLAMA_URL).header("content-type", "application/json").send(&body) {
                Ok(r) => log::info!("ai: local model {model} loaded (HTTP {})", r.status()),
                Err(e) => log::info!("ai: local model not loaded: {e}"),
            }
        })
        .ok();
}

/// Ollama: the reply streams as one JSON object per line (`{"response": "...", "done": false}`).
fn run_local(model: &str, ctx: &Context, generation: &Generation, id: u64, sink: &impl Fn(u64, Event)) -> Option<Event> {
    let body = local_body(model, ctx).to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        // A request that has to load the model first can take a minute.
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into();
    let resp = match agent.post(OLLAMA_URL).header("content-type", "application/json").send(&body) {
        Ok(r) => r,
        // Only a refused connection means Ollama is not running; a timeout means it is busy
        // (e.g. still loading the model).
        Err(ureq::Error::Timeout(_)) => {
            log::warn!("ai local: timeout");
            return Some(Event::Failed(Failure::Other("Ollama took too long to answer".into())));
        }
        Err(e) => {
            log::warn!("ai local: {e}");
            return Some(Event::Failed(Failure::NoLocalServer));
        }
    };
    let status = resp.status().as_u16();
    if status != 200 {
        let text = resp.into_body().read_to_string().unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or(text);
        log::warn!("ai local: HTTP {status}: {message}");
        return Some(Event::Failed(if status == 404 {
            Failure::ModelMissing(model.to_string())
        } else {
            Failure::Other(format!("Ollama error {status}: {message}"))
        }));
    }
    let reader = BufReader::new(resp.into_body().into_reader());
    let mut text = String::new();
    for line in reader.lines() {
        if generation.current() != id {
            return None;
        }
        let Ok(line) = line else { break };
        let Ok(ev) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(e) = ev["error"].as_str() {
            return Some(Event::Failed(Failure::Other(format!("Ollama: {e}"))));
        }
        text.push_str(ev["response"].as_str().unwrap_or(""));
        // Enough lines: stop the model early instead of letting it run on.
        if text.matches('\n').count() >= MAX_LINES {
            break;
        }
        sink(id, Event::Partial(text.clone()));
        if ev["done"] == true {
            break;
        }
    }
    Some(Event::Done(text))
}

fn run(key: &str, model: Model, ctx: &Context, generation: &Generation, id: u64, sink: &impl Fn(u64, Event)) -> Option<Event> {
    let body = request_body(model, ctx).to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut req = agent
        .post(API_URL)
        .header("x-api-key", key)
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json");
    if model == Model::Sonnet {
        req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
    }
    let resp = match req.send(&body) {
        Ok(r) => r,
        Err(e) => return Some(Event::Failed(Failure::Other(format!("no connection to the Anthropic API: {e}")))),
    };
    let status = resp.status().as_u16();
    if status != 200 {
        let text = resp.into_body().read_to_string().unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or(text);
        log::warn!("ai: HTTP {status}: {message}");
        return Some(Event::Failed(match status {
            401 | 403 => Failure::BadKey,
            429 | 529 => Failure::Busy,
            _ => Failure::Other(format!("Claude API error {status}: {message}")),
        }));
    }
    let reader = BufReader::new(resp.into_body().into_reader());
    let mut text = String::new();
    let mut refused = false;
    for line in reader.lines() {
        if generation.current() != id {
            return None; // outdated: the user typed on
        }
        let Ok(line) = line else { break };
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(ev) = serde_json::from_str::<Value>(data.trim()) else { continue };
        match ev["type"].as_str() {
            Some("content_block_delta") if ev["delta"]["type"] == "text_delta" => {
                text.push_str(ev["delta"]["text"].as_str().unwrap_or(""));
                sink(id, Event::Partial(text.clone()));
            }
            Some("message_delta") => {
                if let Some(u) = ev.get("usage") {
                    log::debug!("ai usage: {u}");
                }
                refused |= ev["delta"]["stop_reason"] == "refusal";
            }
            Some("message_start") => log::debug!("ai usage (start): {}", ev["message"]["usage"]),
            Some("error") => {
                let msg = ev["error"]["message"].as_str().unwrap_or("stream error").to_string();
                let busy = ev["error"]["type"] == "overloaded_error";
                return Some(Event::Failed(if busy { Failure::Busy } else { Failure::Other(msg) }));
            }
            _ => {}
        }
    }
    Some(Event::Done(if refused { String::new() } else { text }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(before: &str, after: &str) -> Context {
        Context { file_name: "a.cs".into(), language: "C#".into(), before: before.into(), after: after.into() }
    }

    #[test]
    fn small_files_have_no_cached_block() {
        let body = request_body(Model::Haiku, &ctx("int x = ", ";\n"));
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["text"], "<file name=\"a.cs\" language=\"C#\">\nint x = <CURSOR/>;\n\n</file>");
        assert_eq!(body["thinking"]["type"], "disabled");
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert!(body.get("fallbacks").is_none());
    }

    #[test]
    fn the_cached_head_stays_the_same_while_typing() {
        let lines: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let a = request_body(Model::Sonnet, &ctx(&format!("{lines}fo"), ""));
        let b = request_body(Model::Sonnet, &ctx(&format!("{lines}foo.Bar("), ""));
        let head = |v: &Value| v["messages"][0]["content"][0].clone();
        assert_eq!(head(&a), head(&b));
        assert!(head(&a)["cache_control"].is_object());
        // 200 lines above the cursor, 30 kept out of the head: 150 whole lines in the head.
        assert!(head(&a)["text"].as_str().unwrap().ends_with("line 149\n"));
        assert_eq!(a["fallbacks"], "default");
        assert!(a.get("thinking").is_none());
    }

    #[test]
    fn suggestions_are_cleaned() {
        assert_eq!(clean_suggestion("```csharp\nFoo();\n```", ""), "Foo();");
        assert_eq!(clean_suggestion("x + 1);", ");\n"), "x + 1");
        let long: String = (0..15).map(|i| format!("l{i}\n")).collect();
        assert_eq!(clean_suggestion(&long, "").lines().count(), MAX_LINES);
        assert_eq!(clean_suggestion("  ", ""), "");
    }

    #[test]
    fn local_requests_fill_in_the_middle() {
        let body = local_body("qwen2.5-coder:7b-base", &ctx("int x = ", ";\nint y;\n"));
        assert_eq!(body["prompt"], "<|fim_prefix|>int x = <|fim_suffix|>;\nint y;\n<|fim_middle|>");
        assert_eq!(body["raw"], true);
        assert_eq!(body["stream"], true);
        assert!(body["options"]["stop"].as_array().unwrap().iter().any(|s| s == "<|endoftext|>"));
        let big = "x".repeat(20_000);
        let body = local_body("m", &ctx(&big, &big));
        let markers = FIM_PREFIX.len() + FIM_SUFFIX.len() + FIM_MIDDLE.len();
        assert!(body["prompt"].as_str().unwrap().len() <= LOCAL_BEFORE + LOCAL_AFTER + markers);
    }

    /// What the chat model sent for `for(` (see the bug report): code, repeated braces, a
    /// fence and an explanation.
    #[test]
    fn chat_style_answers_are_cut_to_the_code() {
        let raw = "int i = 0; i < numbers.Count; i++)\n        {\n            count++;\n        }\n    }\n}\n```\n\nIn this code, we define a list";
        let after = ")\n    }\n}\n";
        assert_eq!(
            clean_suggestion(raw, after),
            "int i = 0; i < numbers.Count; i++)\n        {\n            count++;\n        }"
        );
        assert_eq!(clean_suggestion("x<|endoftext|>junk", ""), "x");
    }

    #[test]
    fn backends_have_readable_names() {
        assert_eq!(Backend::Local("qwen2.5-coder:7b-base".into()).display(), "Qwen2.5-Coder 7B (local)");
        assert_eq!(Backend::Claude(Model::Haiku).display(), "Haiku 5.5");
        assert!(!Backend::Local("m".into()).needs_key());
    }

    #[test]
    fn models_parse() {
        assert_eq!(Model::parse("Sonnet"), Model::Sonnet);
        assert_eq!(Model::parse("anything"), Model::Haiku);
    }
}
