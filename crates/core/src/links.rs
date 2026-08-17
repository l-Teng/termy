use std::collections::{HashMap, VecDeque};
#[cfg(windows)]
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedLink {
    pub start_col: usize,
    pub end_col: usize,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedViewportLink {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
    pub target: String,
}

const FILE_URL_CACHE_CAPACITY: usize = 128;

#[derive(Default)]
struct FileUrlLruCache {
    capacity: usize,
    entries: HashMap<String, Option<String>>,
    order: VecDeque<String>,
}

impl FileUrlLruCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<Option<String>> {
        let value = self.entries.get(key).cloned()?;
        self.touch(key);
        Some(value)
    }

    fn insert(&mut self, key: String, value: Option<String>) {
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), value);
            self.touch(&key);
            return;
        }

        self.entries.insert(key.clone(), value);
        self.order.push_back(key);

        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|existing| existing == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
    }
}

fn file_url_cache() -> &'static Mutex<FileUrlLruCache> {
    static CACHE: OnceLock<Mutex<FileUrlLruCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FileUrlLruCache::new(FILE_URL_CACHE_CAPACITY)))
}

fn lookup_cached_file_url(token: &str) -> Option<Option<String>> {
    let mut cache = file_url_cache().lock().ok()?;
    cache.get(token)
}

fn store_cached_file_url(token: &str, resolved: Option<String>) {
    if let Ok(mut cache) = file_url_cache().lock() {
        cache.insert(token.to_string(), resolved);
    }
}

pub fn find_link_in_line(line: &[char], col: usize) -> Option<DetectedLink> {
    if col >= line.len() || line[col].is_whitespace() {
        return None;
    }

    let mut start = col;
    while start > 0 && !line[start - 1].is_whitespace() {
        start -= 1;
    }

    let mut end = col;
    while end + 1 < line.len() && !line[end + 1].is_whitespace() {
        end += 1;
    }

    while start <= end && edge_trim_char(line[start]) {
        start += 1;
    }
    while end >= start && edge_trim_char(line[end]) {
        if end == 0 {
            break;
        }
        end -= 1;
    }

    if start > end {
        return None;
    }

    let token: String = line[start..=end].iter().collect();
    let target = classify_link_token(token.trim_end_matches(':'))?;

    Some(DetectedLink {
        start_col: start,
        end_col: end,
        target,
    })
}

pub fn classify_link_token(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }

    let lower = token.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(token.to_string());
    }

    if lower.starts_with("www.") {
        return Some(format!("https://{token}"));
    }

    if lower.starts_with("file://") {
        return normalize_file_url_token(token);
    }

    if looks_like_file_path(token) {
        return canonicalize_path_to_file_url(token);
    }

    if is_ipv4_with_optional_port_and_path(token) || looks_like_domain(token) {
        return Some(format!("http://{token}"));
    }

    None
}

fn normalize_file_url_token(token: &str) -> Option<String> {
    let raw_path = token.get("file://".len()..)?;
    let local_path = extract_local_path_from_file_url(raw_path)?;
    canonicalize_path_to_file_url(&local_path)
}

#[cfg(unix)]
fn extract_local_path_from_file_url(raw_path: &str) -> Option<String> {
    if raw_path.starts_with('/') {
        return Some(raw_path.to_string());
    }

    let (host, path) = raw_path.split_once('/')?;
    if host.eq_ignore_ascii_case("localhost") {
        return Some(format!("/{path}"));
    }

    None
}

#[cfg(windows)]
fn extract_local_path_from_file_url(raw_path: &str) -> Option<String> {
    if let Some(stripped) = raw_path.strip_prefix('/') {
        if has_windows_drive_prefix(stripped) {
            return Some(stripped.to_string());
        }
    }

    if has_windows_drive_prefix(raw_path) || Path::new(raw_path).is_absolute() {
        return Some(raw_path.to_string());
    }

    let (host, path) = raw_path.split_once('/')?;
    if !host.eq_ignore_ascii_case("localhost") {
        return None;
    }

    if let Some(stripped) = path.strip_prefix('/') {
        if has_windows_drive_prefix(stripped) {
            return Some(stripped.to_string());
        }
    }

    if has_windows_drive_prefix(path) || Path::new(path).is_absolute() {
        return Some(path.to_string());
    }

    None
}

#[cfg(not(any(unix, windows)))]
fn extract_local_path_from_file_url(_: &str) -> Option<String> {
    None
}

fn canonicalize_path_to_file_url(token: &str) -> Option<String> {
    let normalized_key = strip_line_col_suffix(token);
    if normalized_key.is_empty() {
        return None;
    }

    if let Some(cached) = lookup_cached_file_url(normalized_key) {
        return cached;
    }

    let resolved = canonicalize_path_to_file_url_uncached(normalized_key);
    store_cached_file_url(normalized_key, resolved.clone());
    resolved
}

#[cfg(unix)]
fn canonicalize_path_to_file_url_uncached(token: &str) -> Option<String> {
    let raw_path = strip_line_col_suffix(token);
    if raw_path.is_empty() {
        return None;
    }

    let path = expand_tilde_path(raw_path).unwrap_or_else(|| PathBuf::from(raw_path));
    let canonical = std::fs::canonicalize(path).ok()?;
    let canonical = canonical.to_string_lossy().replace('\\', "/");
    if !canonical.starts_with('/') {
        return None;
    }

    Some(format!("file:///{}", canonical.trim_start_matches('/')))
}

#[cfg(unix)]
fn expand_tilde_path(path: &str) -> Option<PathBuf> {
    let remainder = path.strip_prefix("~/")?;
    let home = dirs::home_dir()?;
    Some(home.join(remainder))
}

#[cfg(windows)]
fn canonicalize_path_to_file_url_uncached(token: &str) -> Option<String> {
    let mut raw_path = strip_line_col_suffix(token);
    if raw_path.is_empty() {
        return None;
    }

    if let Some(stripped) = raw_path.strip_prefix('/') {
        if has_windows_drive_prefix(stripped) {
            raw_path = stripped;
        }
    }

    if !has_windows_drive_prefix(raw_path) && !Path::new(raw_path).is_absolute() {
        return None;
    }

    let canonical = std::fs::canonicalize(raw_path).ok()?;
    let canonical = canonical.to_string_lossy();
    let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    let canonical = canonical.replace('\\', "/");

    if !has_windows_drive_prefix(&canonical) {
        return None;
    }

    let drive = canonical.chars().next()?.to_ascii_uppercase();
    let path = canonical[2..].trim_start_matches('/');

    if path.is_empty() {
        Some(format!("file:///{drive}:/"))
    } else {
        Some(format!("file:///{drive}:/{path}"))
    }
}

#[cfg(not(any(unix, windows)))]
fn canonicalize_path_to_file_url_uncached(_: &str) -> Option<String> {
    None
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn edge_trim_char(c: char) -> bool {
    matches!(
        c,
        '\'' | '"'
            | '`'
            | ','
            | '.'
            | ';'
            | '!'
            | '?'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '<'
            | '>'
    )
}

fn is_ipv4_with_optional_port_and_path(input: &str) -> bool {
    let host_port = input.split('/').next().unwrap_or(input);
    let (host, port) = if let Some((host, port)) = host_port.rsplit_once(':') {
        (host, Some(port))
    } else {
        (host_port, None)
    };

    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    if octets
        .iter()
        .any(|octet| octet.is_empty() || octet.parse::<u8>().is_err())
    {
        return false;
    }

    if let Some(port) = port {
        if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if port.parse::<u16>().is_err() {
            return false;
        }
    }

    true
}

fn looks_like_domain(input: &str) -> bool {
    let host_port = input.split('/').next().unwrap_or(input);
    let (host, port) = if let Some((host, port)) = host_port.rsplit_once(':') {
        (host, Some(port))
    } else {
        (host_port, None)
    };

    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    if !host.contains('.') {
        return false;
    }

    for label in host.split('.') {
        if label.is_empty() {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }

    if let Some(port) = port {
        if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if port.parse::<u16>().is_err() {
            return false;
        }
    }

    true
}

fn looks_like_file_path(input: &str) -> bool {
    // Strip optional line:col suffix (e.g., "file.rs:42" or "file.rs:42:10")
    let path = strip_line_col_suffix(input);

    if path.is_empty() {
        return false;
    }

    // Absolute Unix paths
    if path.starts_with('/') {
        return has_path_like_structure(path);
    }

    // Home directory paths
    if path.starts_with("~/") {
        return has_path_like_structure(path);
    }

    // Relative paths starting with ./ or ../
    if path.starts_with("./") || path.starts_with("../") {
        return has_path_like_structure(path);
    }

    // Windows absolute paths (C:\, D:\, etc.)
    if path.len() >= 3 {
        let bytes = path.as_bytes();
        if has_windows_drive_prefix(path) && (bytes[2] == b'\\' || bytes[2] == b'/') {
            return has_path_like_structure(path);
        }
    }

    false
}

fn strip_line_col_suffix(input: &str) -> &str {
    // Handle patterns like "file.rs:42" or "file.rs:42:10"
    let mut path = input;

    // Try to strip :col suffix first
    if let Some(colon_pos) = path.rfind(':') {
        let suffix = &path[colon_pos + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            path = &path[..colon_pos];
            // Try to strip :line suffix
            if let Some(colon_pos2) = path.rfind(':') {
                let suffix2 = &path[colon_pos2 + 1..];
                if !suffix2.is_empty() && suffix2.chars().all(|c| c.is_ascii_digit()) {
                    path = &path[..colon_pos2];
                }
            }
        }
    }

    path
}

fn has_path_like_structure(path: &str) -> bool {
    // Must contain at least one path separator or have a file extension
    let has_separator = path.contains('/') || path.contains('\\');
    let has_extension = path.rfind('.').is_some_and(|dot_pos| {
        let after_dot = &path[dot_pos + 1..];
        !after_dot.is_empty()
            && after_dot.len() <= 10
            && after_dot.chars().all(|c| c.is_ascii_alphanumeric())
    });

    has_separator || has_extension
}

#[cfg(test)]
mod tests {
    use super::{FileUrlLruCache, classify_link_token};
    use crate::{Terminal, TerminalSize};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn term_with_input(cols: u16, rows: u16, input: &[u8]) -> Terminal {
        let size = TerminalSize {
            cols,
            rows,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let term = Terminal::new_display(size, None);
        term.feed_output(input);
        term
    }

    #[test]
    fn osc8_hyperlink_detected_under_each_link_cell() {
        let term = term_with_input(
            20,
            2,
            b"\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\ after",
        );

        for col in 0..4 {
            let link = term
                .hyperlink_at(0, col)
                .expect("hyperlinked cell should report the OSC 8 target");
            assert_eq!(link.start_col, 0);
            assert_eq!(link.end_col, 3);
            assert_eq!(link.target, "https://example.com");
        }
    }

    #[test]
    fn osc8_hyperlink_absent_outside_link_run() {
        let term = term_with_input(
            20,
            2,
            b"\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\ after",
        );

        assert_eq!(term.hyperlink_at(0, 4), None);
        assert_eq!(term.hyperlink_at(0, 6), None);
        assert_eq!(term.hyperlink_at(1, 0), None);
    }

    #[test]
    fn osc8_adjacent_distinct_links_do_not_merge() {
        let term = term_with_input(
            20,
            2,
            b"\x1b]8;;https://a.example\x1b\\aa\x1b]8;;https://b.example\x1b\\bb\x1b]8;;\x1b\\",
        );

        let first = term.hyperlink_at(0, 1).expect("first link");
        assert_eq!((first.start_col, first.end_col), (0, 1));
        assert_eq!(first.target, "https://a.example");

        let second = term.hyperlink_at(0, 2).expect("second link");
        assert_eq!((second.start_col, second.end_col), (2, 3));
        assert_eq!(second.target, "https://b.example");
    }

    #[test]
    fn osc8_same_id_split_runs_only_cover_contiguous_cells() {
        // Same link id interrupted by a plain cell: hover should only span the
        // contiguous run under the cursor, not jump across the gap.
        let term = term_with_input(
            20,
            2,
            b"\x1b]8;id=x;https://example.com\x1b\\ab\x1b]8;;\x1b\\-\x1b]8;id=x;https://example.com\x1b\\cd\x1b]8;;\x1b\\",
        );

        let left = term.hyperlink_at(0, 0).expect("left run");
        assert_eq!((left.start_col, left.end_col), (0, 1));
        let right = term.hyperlink_at(0, 3).expect("right run");
        assert_eq!((right.start_col, right.end_col), (3, 4));
    }

    #[test]
    fn plain_text_has_no_hyperlink() {
        let term = term_with_input(20, 2, b"https://example.com");
        assert_eq!(term.hyperlink_at(0, 0), None);
    }

    #[test]
    fn detected_url_spans_soft_wrapped_rows() {
        let term = term_with_input(10, 4, b"go https://example.com/path");

        for (row, col) in [(0, 4), (1, 5), (2, 4)] {
            let link = term
                .link_at(row, col)
                .expect("each wrapped URL segment should resolve");
            assert_eq!((link.start_row, link.start_col), (0, 3));
            assert_eq!((link.end_row, link.end_col), (2, 6));
            assert_eq!(link.target, "https://example.com/path");
        }
    }

    #[test]
    fn detected_url_does_not_cross_hard_line_break() {
        let term = term_with_input(20, 3, b"https://a.co\r\nsecond.example");

        let second = term
            .link_at(1, 3)
            .expect("second line should be its own domain");
        assert_eq!((second.start_row, second.start_col), (1, 0));
        assert_eq!((second.end_row, second.end_col), (1, 13));
        assert_eq!(second.target, "http://second.example");
    }

    #[test]
    fn osc8_hyperlink_span_follows_soft_wrap() {
        let term = term_with_input(
            5,
            3,
            b"\x1b]8;;https://example.com\x1b\\read-more\x1b]8;;\x1b\\",
        );

        let link = term
            .link_at(1, 2)
            .expect("wrapped OSC 8 text should resolve as one link");
        assert_eq!((link.start_row, link.start_col), (0, 0));
        assert_eq!((link.end_row, link.end_col), (1, 3));
        assert_eq!(link.target, "https://example.com");
    }

    fn unique_temp_path(file_name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("termy-links-{nonce}-{file_name}"))
    }

    #[test]
    fn absolute_file_paths_emit_well_formed_file_urls() {
        let file_path = unique_temp_path("sample.txt");
        fs::write(&file_path, "sample").expect("write temp file");

        let token = file_path.to_string_lossy();
        let link = classify_link_token(&token).expect("file path should produce a file URL");

        assert!(link.starts_with("file:///"));
        assert!(!link.contains('\\'));

        #[cfg(unix)]
        {
            let canonical = fs::canonicalize(&file_path).expect("canonicalize temp file");
            let canonical = canonical.to_string_lossy();
            assert_eq!(
                link,
                format!("file:///{}", canonical.trim_start_matches('/'))
            );
        }

        let _ = fs::remove_file(file_path);
    }

    #[test]
    fn file_path_line_col_suffix_is_ignored_for_url_generation() {
        let file_path = unique_temp_path("with-line-col.rs");
        fs::write(&file_path, "fn main() {}").expect("write temp file");

        let token = file_path.to_string_lossy();
        let expected = classify_link_token(&token).expect("base file path should classify");
        let with_suffix = format!("{token}:42:10");

        assert_eq!(classify_link_token(&with_suffix), Some(expected));

        let _ = fs::remove_file(file_path);
    }

    #[test]
    fn malformed_file_urls_are_rejected() {
        assert_eq!(classify_link_token("file://relative/path.txt"), None);
    }

    #[test]
    fn non_canonicalizable_file_paths_are_rejected() {
        let missing_path = unique_temp_path("missing-file.txt");
        let token = missing_path.to_string_lossy();

        assert_eq!(classify_link_token(&token), None);
    }

    #[test]
    fn file_url_lru_cache_evicts_old_entries() {
        let mut cache = FileUrlLruCache::new(2);
        cache.insert("a".to_string(), Some("file:///a".to_string()));
        cache.insert("b".to_string(), Some("file:///b".to_string()));
        assert_eq!(cache.get("a"), Some(Some("file:///a".to_string())));
        cache.insert("c".to_string(), Some("file:///c".to_string()));

        assert_eq!(cache.get("a"), Some(Some("file:///a".to_string())));
        assert_eq!(cache.get("b"), None);
        assert_eq!(cache.get("c"), Some(Some("file:///c".to_string())));
    }
}
