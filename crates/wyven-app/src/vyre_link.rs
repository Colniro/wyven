//! Opening links from the terminal in Vyre (the browser): Ctrl+click on a URL.
//!
//! Wyven finds `vyre.exe` on the PATH (Vyre adds itself to the user PATH on its first start,
//! which a running Wyven does not see yet, so the registry is read as well). Vyre and Wyven then
//! share the screen: Wyven on the left half, Vyre on the right. Without Vyre the link opens in the
//! default browser.

use std::path::PathBuf;

use winit::window::Window;

/// URLs in a line of terminal text: (first column, end column (exclusive), url).
/// Columns are character positions.
pub fn find_links(line: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..chars.len().min(i + 8)].iter().collect();
        let starts = rest.starts_with("https://") || rest.starts_with("http://");
        let boundary = i == 0 || !chars[i - 1].is_alphanumeric();
        if !(starts && boundary) {
            i += 1;
            continue;
        }
        let mut end = i;
        while end < chars.len() && !chars[end].is_whitespace() && !"\"'<>`{}|\\^".contains(chars[end]) {
            end += 1;
        }
        // Trailing punctuation belongs to the sentence, not the URL; an unmatched ")" too.
        while end > i && ".,;:!?".contains(chars[end - 1]) {
            end -= 1;
        }
        let text: String = chars[i..end].iter().collect();
        let opens = text.matches('(').count();
        let mut url = text.clone();
        let mut closes = url.matches(')').count();
        while closes > opens && url.ends_with(')') {
            url.pop();
            end -= 1;
            closes -= 1;
        }
        if url.len() > "https://".len() {
            out.push((i, end, url));
        }
        i = end.max(i + 1);
    }
    out
}

/// `vyre.exe` on the PATH of this process or the (possibly newer) user PATH in the registry.
pub fn find_vyre() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(user) = user_path() {
        dirs.extend(user.split(';').filter(|d| !d.trim().is_empty()).map(PathBuf::from));
    }
    dirs.into_iter().map(|d| d.join("vyre.exe")).find(|p| p.is_file())
}

#[cfg(windows)]
fn user_path() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_ANY, RegGetValueW};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let (key, value) = (wide("Environment"), wide("Path"));
    let mut size = 0u32;
    // SAFETY: plain registry reads into a buffer of the reported size.
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_ANY, std::ptr::null_mut(), std::ptr::null_mut(), &mut size);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2];
        let ok = RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_ANY, std::ptr::null_mut(), buf.as_mut_ptr() as _, &mut size);
        (ok == 0).then(|| String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string())
    }
}

#[cfg(not(windows))]
fn user_path() -> Option<String> {
    None
}

/// The work area (screen without taskbar) of the monitor the window is on: [x, y, width, height].
#[cfg(windows)]
pub fn work_area(window: &Window) -> Option<[i32; 4]> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::Win32(h) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: valid window handle; MONITORINFO with its size set.
    unsafe {
        let monitor = MonitorFromWindow(h.hwnd.get() as _, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        let r = info.rcWork;
        Some([r.left, r.top, r.right - r.left, r.bottom - r.top])
    }
}

#[cfg(not(windows))]
pub fn work_area(_window: &Window) -> Option<[i32; 4]> {
    None
}

/// Opens the URL in Vyre next to Wyven (Wyven moves to the left half). Returns a status message.
pub fn open(url: &str, window: &Window) -> String {
    let Some(vyre) = find_vyre() else {
        return match std::process::Command::new("explorer.exe").arg(url).spawn() {
            Ok(_) => format!("Vyre not found – opened {url} in the default browser"),
            Err(e) => format!("Could not open {url}: {e}"),
        };
    };
    let area = work_area(window);
    let mut cmd = std::process::Command::new(&vyre);
    cmd.arg("open").arg(url);
    if let Some([x, y, w, h]) = area {
        cmd.arg("--neben").arg(format!("{x},{y},{w},{h}"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    if let Err(e) = cmd.spawn() {
        return format!("Could not start Vyre: {e}");
    }
    if let Some([x, y, w, h]) = area {
        // Wyven takes the left half; the window frame is outside the inner size.
        window.set_maximized(false);
        let outer = window.outer_size();
        let inner = window.inner_size();
        let (fw, fh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
        window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(((w / 2) as u32).saturating_sub(fw), (h as u32).saturating_sub(fh)));
    }
    format!("Opened {url} in Vyre")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_urls_in_terminal_lines() {
        let line = "  ➜  Local:   http://localhost:5173/ (press h for help).";
        let links = find_links(line);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].2, "http://localhost:5173/");
        let chars: Vec<char> = line.chars().collect();
        let (s, e) = (links[0].0, links[0].1);
        assert_eq!(chars[s..e].iter().collect::<String>(), "http://localhost:5173/");

        let wiki = find_links("see https://de.wikipedia.org/wiki/Rust_(Programmiersprache), ok");
        assert_eq!(wiki[0].2, "https://de.wikipedia.org/wiki/Rust_(Programmiersprache)");
        assert_eq!(find_links("(https://a.de/x)")[0].2, "https://a.de/x");
        assert_eq!(find_links("a https://x.org and http://y.org").len(), 2);
        assert!(find_links("nohttps://x.org").is_empty());
        assert!(find_links("https://").is_empty());
    }
}
