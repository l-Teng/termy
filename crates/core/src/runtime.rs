use crate::frame::{
    TerminalRenderDamageSnapshot, TerminalRenderRead, TerminalViewportMetadata, TermyFrame,
    TermyFrameUpdate,
};
use crate::keyboard::TerminalKeyboardMode;
use crate::kitty_graphics::{
    KittyGraphicsRenderPlacement, KittyGraphicsScreen, KittyGraphicsState,
};
#[cfg(unix)]
use crate::locale::{Utf8LocaleOverridePlan, preferred_utf8_locale, utf8_locale_override_plan};
use crate::mouse_protocol::TerminalMouseMode;
use crate::path_env::normalized_path_env;
use crate::protocol::{TerminalQueryColors, TerminalReplyHost};
use crate::search::{TermySearchMatch, TermySearchOptions, TermySharedSearchMatch};
use crate::shell_integration::ProgressState;
use flume::Sender;
#[cfg(not(target_os = "windows"))]
use std::path::Path;
use std::{collections::HashMap, env, io, path::PathBuf, sync::Arc};

#[derive(Debug, Clone)]
pub struct TabTitleShellIntegration {
    pub enabled: bool,
    pub explicit_prefix: String,
}

const DEFAULT_TERM: &str = "xterm-256color";
const DEFAULT_COLORTERM: &str = "truecolor";
const TERMY_TERM_PROGRAM: &str = "termy";
const GHOSTTY_COMPAT_TERM_PROGRAM: &str = "ghostty";
const GHOSTTY_COMPAT_TERM_PROGRAM_VERSION: &str = "1.2.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingDirFallback {
    Home,
    Process,
}

#[allow(clippy::derivable_impls)]
impl Default for WorkingDirFallback {
    fn default() -> Self {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Self::Home
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Process
        }
    }
}

const DEFAULT_SCROLLBACK_HISTORY: usize = 1000;

/// Upper clamp on scrollback lines, enforced at the point the value is applied
/// to the live grid. The config-file parser (`config_core`) already bounds this
/// at parse time, but the runtime/FFI setters (`with_scrollback_history`,
/// `set_scrollback_history`) and directly-constructed `TerminalRuntimeConfig`s
/// bypass that parser, so the core must self-defend: each pane eagerly grows its
/// scrollback toward this cap, so an unbounded value plus hostile output is an
/// unbounded memory leak. Kept in parity with `config_core`'s constant of the
/// same name.
pub const MAX_TERMINAL_SCROLLBACK_HISTORY: usize = 20_000;

/// Upper clamp on terminal dimensions. Real displays never approach this (an 8K
/// display at a 4px font is ~1900 columns); it exists only to stop a buggy or
/// hostile embedder from requesting a multi-gigabyte grid — `u16::MAX` on both
/// axes is ~4.3 billion cells. Clamping each axis bounds the worst-case grid
/// (and the frame snapshot allocated from it) to `MAX_TERMINAL_COLS` ×
/// `MAX_TERMINAL_ROWS` cells.
const MAX_TERMINAL_COLS: u16 = 4096;
const MAX_TERMINAL_ROWS: u16 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowsShell {
    #[default]
    Cmd,
    PowerShell,
    PowerShellCore,
    GitBash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCursorStyle {
    Line,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursorState {
    pub col: usize,
    pub row: usize,
    pub style: TerminalCursorStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    pub scrollback_history: usize,
    pub default_cursor_style: TerminalCursorStyle,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            scrollback_history: DEFAULT_SCROLLBACK_HISTORY,
            default_cursor_style: TerminalCursorStyle::Block,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TerminalRuntimeConfig {
    pub shell: Option<String>,
    pub windows_shell: WindowsShell,
    pub term: String,
    pub colorterm: Option<String>,
    pub environment: HashMap<String, String>,
    pub query_colors: TerminalQueryColors,
    pub working_dir_fallback: WorkingDirFallback,
    pub scrollback_history: usize,
    pub default_cursor_style: TerminalCursorStyle,
}

/// Selects what owns a newly-created PTY.
///
/// `ShellCommand` preserves the existing shell-evaluated startup-command API.
/// Structured tools such as OpenSSH must use `Program`, which sends each
/// argument directly to the child without routing through a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalLaunch {
    ShellCommand(String),
    Program { program: String, args: Vec<String> },
}

/// Why the runtime selected its active terminal engine.
///
/// This is diagnostic state only. Engine selection remains private to core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEngineSelectionReason {
    TmonDefault,
    DisplayDefault,
    /// Legacy diagnostic token retained for Rust source compatibility; never emitted.
    ForcedAlacritty,
    /// Legacy diagnostic token retained for Rust source compatibility; never emitted.
    TmonUnavailable,
    /// Legacy diagnostic token retained for Rust source compatibility; never emitted.
    TmonInitializationFailure,
    /// Legacy diagnostic token retained for Rust source compatibility; never emitted.
    TestOverride,
}

impl TerminalEngineSelectionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TmonDefault => "tmon-default",
            Self::DisplayDefault => "display-default",
            Self::ForcedAlacritty => "forced-alacritty",
            Self::TmonUnavailable => "tmon-unavailable",
            Self::TmonInitializationFailure => "tmon-initialization-failure",
            Self::TestOverride => "test-override",
        }
    }
}

impl std::fmt::Display for TerminalEngineSelectionReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The actual engine plus the reason it was selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEngineDiagnostics {
    pub engine: &'static str,
    pub selection_reason: TerminalEngineSelectionReason,
    pub fallback_detail: Option<String>,
}

impl Default for TerminalRuntimeConfig {
    fn default() -> Self {
        Self {
            shell: None,
            windows_shell: WindowsShell::default(),
            term: DEFAULT_TERM.to_string(),
            colorterm: Some(DEFAULT_COLORTERM.to_string()),
            environment: HashMap::new(),
            query_colors: TerminalQueryColors::default(),
            working_dir_fallback: WorkingDirFallback::default(),
            scrollback_history: DEFAULT_SCROLLBACK_HISTORY,
            default_cursor_style: TerminalCursorStyle::Block,
        }
    }
}

impl TerminalRuntimeConfig {
    pub fn resolved_shell_program(&self) -> String {
        default_shell_launch(self).program
    }
}

impl TerminalOptions {
    pub fn with_scrollback_history(self, scrollback_history: usize) -> Self {
        Self {
            scrollback_history,
            ..self
        }
    }
}

impl TerminalRuntimeConfig {
    pub fn term_options(&self) -> TerminalOptions {
        TerminalOptions {
            scrollback_history: self.scrollback_history,
            default_cursor_style: self.default_cursor_style,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct KittyGraphicsScrollRegion {
    top: usize,
    bottom: Option<usize>,
}

impl Default for KittyGraphicsScrollRegion {
    fn default() -> Self {
        Self {
            top: 1,
            bottom: None,
        }
    }
}

impl KittyGraphicsScrollRegion {
    fn bounds(self, screen_lines: usize) -> (usize, usize) {
        let top = self.top.saturating_sub(1).min(screen_lines);
        let bottom = self.bottom.unwrap_or(screen_lines).min(screen_lines);
        (top, bottom)
    }

    fn covers_full_screen(self, screen_lines: usize) -> bool {
        self.bounds(screen_lines) == (0, screen_lines)
    }

    fn set(&mut self, top: usize, bottom: Option<usize>, screen_lines: usize) {
        // Resolve an omitted bottom before validating, then
        // clamp the accepted region to the screen during use.
        if top >= bottom.unwrap_or(screen_lines) {
            return;
        }
        self.top = top;
        self.bottom = bottom;
    }

    fn reset(&mut self) {
        self.top = 1;
        self.bottom = None;
    }
}

/// Tracks DECSTBM alongside the real terminal parser so Kitty commands can
/// choose a scroll-safe cursor policy without reaching into engine internals.
#[derive(Default)]
pub struct KittyGraphicsCursorTracker {
    region: KittyGraphicsScrollRegion,
}

impl KittyGraphicsCursorTracker {
    pub fn region_covers_full_screen(&self, screen_lines: usize) -> bool {
        self.region.covers_full_screen(screen_lines)
    }

    pub fn reset_scroll_region(&mut self) {
        self.region.reset();
    }

    pub fn set_scroll_region(&mut self, top: usize, bottom: Option<usize>, screen_lines: usize) {
        self.region.set(top, bottom, screen_lines);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum KittyGraphicsTextEffect {
    EnteredAlternateScreen,
    TerminalReset,
    PreservePrimaryAcrossPartialHistoryGrowth(usize),
    ScrollUpWithoutHistory {
        screen: KittyGraphicsScreen,
        lines: usize,
    },
    ClearViewport {
        screen: KittyGraphicsScreen,
        history_size: usize,
        rows: usize,
        cols: usize,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KittyGraphicsTextEffects {
    effects: Vec<KittyGraphicsTextEffect>,
}

impl KittyGraphicsTextEffects {
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn apply_to(self, graphics: &mut KittyGraphicsState) -> bool {
        let mut changed = false;
        for effect in self.effects {
            changed |= match effect {
                KittyGraphicsTextEffect::EnteredAlternateScreen => {
                    graphics.clear_visible_on_screen(KittyGraphicsScreen::Alternate)
                }
                KittyGraphicsTextEffect::TerminalReset => {
                    let primary = graphics.clear_visible_on_screen(KittyGraphicsScreen::Primary);
                    let alternate =
                        graphics.clear_visible_on_screen(KittyGraphicsScreen::Alternate);
                    primary || alternate
                }
                KittyGraphicsTextEffect::PreservePrimaryAcrossPartialHistoryGrowth(lines) => {
                    graphics.preserve_primary_placements_across_partial_history_growth(lines)
                }
                KittyGraphicsTextEffect::ScrollUpWithoutHistory { screen, lines } => {
                    graphics.scroll_up_without_history_on_screen(lines, screen)
                }
                KittyGraphicsTextEffect::ClearViewport {
                    screen,
                    history_size,
                    rows,
                    cols,
                } => graphics.clear_viewport_on_screen(screen, history_size, rows, cols),
            };
        }
        changed
    }
}

fn login_shell_args(shell_path: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let _ = shell_path;
        Vec::new()
    }

    // On macOS, terminals conventionally launch login shells so that the user's
    // PATH and environment (set up in ~/.bash_profile, ~/.zprofile, etc.) are
    // available.  Pass both -i (interactive) and -l (login).
    #[cfg(target_os = "macos")]
    match Path::new(shell_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("bash" | "zsh" | "fish") => vec!["-i".to_string(), "-l".to_string()],
        _ => Vec::new(),
    }

    // On Linux (and other non-macOS Unix), the user is already in a login
    // session, so sourcing all login scripts on every terminal open adds
    // unnecessary startup latency.  Launch an interactive non-login shell
    // instead, which is the convention used by Tmon and other Linux
    // terminal emulators.
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    match Path::new(shell_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("bash" | "zsh" | "fish") => vec!["-i".to_string()],
        _ => Vec::new(),
    }
}

/// The executable and argument vector selected for a terminal PTY.
///
/// All terminal engines must use [`resolve_terminal_launch`] instead of
/// independently interpreting [`TerminalRuntimeConfig`] or [`TerminalLaunch`].
/// This keeps platform shell selection, login-shell arguments, and startup
/// command handling identical across engines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTerminalLaunch {
    pub program: String,
    pub args: Vec<String>,
}

#[cfg(target_os = "windows")]
fn windows_cmd_path() -> String {
    if let Ok(comspec) = env::var("COMSPEC")
        && !comspec.trim().is_empty()
    {
        return comspec;
    }
    "C:\\Windows\\System32\\cmd.exe".to_string()
}

#[cfg(target_os = "windows")]
fn windows_git_bash_path() -> String {
    let mut candidates = Vec::new();
    if let Ok(program_files) = env::var("ProgramFiles")
        && !program_files.trim().is_empty()
    {
        candidates.push(PathBuf::from(program_files).join("Git\\bin\\bash.exe"));
    }
    if let Ok(program_files_x86) = env::var("ProgramFiles(x86)")
        && !program_files_x86.trim().is_empty()
    {
        candidates.push(PathBuf::from(program_files_x86).join("Git\\bin\\bash.exe"));
    }
    if let Ok(local_app_data) = env::var("LOCALAPPDATA")
        && !local_app_data.trim().is_empty()
    {
        candidates.push(PathBuf::from(local_app_data).join("Programs\\Git\\bin\\bash.exe"));
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map_or_else(|| "bash.exe".to_string(), |path| path.display().to_string())
}

#[cfg(any(not(target_os = "windows"), test))]
fn resolve_shell_path(configured_shell: Option<&str>) -> String {
    if let Some(shell) = configured_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())
    {
        return shell.to_string();
    }

    if let Ok(shell) = env::var("SHELL")
        && !shell.trim().is_empty()
    {
        return shell;
    }

    #[cfg(target_os = "windows")]
    {
        windows_cmd_path()
    }

    #[cfg(target_os = "macos")]
    {
        "/bin/zsh".to_string()
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        "/bin/bash".to_string()
    }
}

#[cfg(target_os = "windows")]
fn windows_shell_launch(windows_shell: WindowsShell) -> ResolvedTerminalLaunch {
    match windows_shell {
        WindowsShell::Cmd => ResolvedTerminalLaunch {
            program: windows_cmd_path(),
            args: Vec::new(),
        },
        WindowsShell::PowerShell => ResolvedTerminalLaunch {
            program: "powershell.exe".to_string(),
            args: vec!["-NoLogo".to_string()],
        },
        WindowsShell::PowerShellCore => ResolvedTerminalLaunch {
            program: "pwsh.exe".to_string(),
            args: vec!["-NoLogo".to_string()],
        },
        WindowsShell::GitBash => ResolvedTerminalLaunch {
            program: windows_git_bash_path(),
            args: vec!["--login".to_string(), "-i".to_string()],
        },
    }
}

#[cfg(target_os = "windows")]
fn windows_startup_command_shell(
    windows_shell: WindowsShell,
    command: &str,
) -> ResolvedTerminalLaunch {
    match windows_shell {
        WindowsShell::Cmd => ResolvedTerminalLaunch {
            program: windows_cmd_path(),
            args: vec!["/C".to_string(), command.to_string()],
        },
        WindowsShell::PowerShell => ResolvedTerminalLaunch {
            program: "powershell.exe".to_string(),
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        },
        WindowsShell::PowerShellCore => ResolvedTerminalLaunch {
            program: "pwsh.exe".to_string(),
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        },
        WindowsShell::GitBash => ResolvedTerminalLaunch {
            program: windows_git_bash_path(),
            args: vec!["-lc".to_string(), command.to_string()],
        },
    }
}

fn configured_shell_launch(configured_shell: Option<&str>) -> Option<ResolvedTerminalLaunch> {
    let shell_path = configured_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())?;
    Some(ResolvedTerminalLaunch {
        program: shell_path.to_string(),
        args: login_shell_args(shell_path),
    })
}

fn default_shell_launch(runtime_config: &TerminalRuntimeConfig) -> ResolvedTerminalLaunch {
    if let Some(launch) = configured_shell_launch(runtime_config.shell.as_deref()) {
        return launch;
    }

    #[cfg(target_os = "windows")]
    {
        windows_shell_launch(runtime_config.windows_shell)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let shell_path = resolve_shell_path(None);
        ResolvedTerminalLaunch {
            program: shell_path.clone(),
            args: login_shell_args(&shell_path),
        }
    }
}

pub fn resolve_terminal_launch(
    runtime_config: &TerminalRuntimeConfig,
    launch: Option<&TerminalLaunch>,
) -> anyhow::Result<ResolvedTerminalLaunch> {
    if let Some(TerminalLaunch::Program { program, args }) = launch {
        anyhow::ensure!(
            !program.trim().is_empty(),
            "terminal program cannot be empty"
        );
        anyhow::ensure!(
            !program.contains('\0') && !args.iter().any(|arg| arg.contains('\0')),
            "terminal program and arguments cannot contain NUL bytes"
        );
        return Ok(ResolvedTerminalLaunch {
            program: program.clone(),
            args: args.clone(),
        });
    }

    if let Some(command) = launch.and_then(|launch| match launch {
        TerminalLaunch::ShellCommand(command) => {
            Some(command.trim()).filter(|command| !command.is_empty())
        }
        TerminalLaunch::Program { .. } => None,
    }) {
        #[cfg(unix)]
        {
            return Ok(ResolvedTerminalLaunch {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), command.to_string()],
            });
        }

        #[cfg(target_os = "windows")]
        {
            if runtime_config
                .shell
                .as_deref()
                .map(str::trim)
                .is_some_and(|shell| !shell.is_empty())
            {
                return Ok(ResolvedTerminalLaunch {
                    program: "cmd.exe".to_string(),
                    args: vec!["/C".to_string(), command.to_string()],
                });
            }

            return Ok(windows_startup_command_shell(
                runtime_config.windows_shell,
                command,
            ));
        }
    }

    Ok(default_shell_launch(runtime_config))
}

fn user_home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(user_profile) = env::var("USERPROFILE")
            && !user_profile.trim().is_empty()
        {
            return Some(PathBuf::from(user_profile));
        }

        if let (Ok(home_drive), Ok(home_path)) = (env::var("HOMEDRIVE"), env::var("HOMEPATH"))
            && !home_drive.trim().is_empty()
            && !home_path.trim().is_empty()
        {
            return Some(PathBuf::from(format!("{home_drive}{home_path}")));
        }
    }

    if let Ok(home) = env::var("HOME")
        && !home.trim().is_empty()
    {
        return Some(PathBuf::from(home));
    }

    None
}

/// Build the child-process environment shared by native terminal engines.
///
/// Keeping this at the engine boundary prevents experimental backends from
/// drifting on terminal identity, PATH normalization, shell integration, or
/// Unix UTF-8 locale repair.
pub fn terminal_environment_overrides(
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: &TerminalRuntimeConfig,
) -> HashMap<String, String> {
    let mut env_overrides = HashMap::new();

    if let Some(path) = normalized_path_env(
        env::var_os("PATH")
            .or_else(|| env::var_os("Path"))
            .as_deref(),
    ) {
        env_overrides.insert("PATH".to_string(), path);
    }

    let term = runtime_config.term.trim();
    let term = if term.is_empty() { DEFAULT_TERM } else { term };
    env_overrides.insert("TERM".to_string(), term.to_string());

    if let Some(colorterm) = runtime_config
        .colorterm
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        env_overrides.insert("COLORTERM".to_string(), colorterm.to_string());
    }

    // Claude Code and similar CLIs gate terminal progress escape sequences on
    // known terminal identities. Termy supports Ghostty's OSC progress
    // protocol, so advertise that compatibility to child processes while
    // keeping TERM conservative for terminfo.
    env_overrides.insert(
        "TERM_PROGRAM".to_string(),
        GHOSTTY_COMPAT_TERM_PROGRAM.to_string(),
    );
    env_overrides.insert(
        "TERM_PROGRAM_VERSION".to_string(),
        GHOSTTY_COMPAT_TERM_PROGRAM_VERSION.to_string(),
    );
    env_overrides.insert(
        "TERMY_TERM_PROGRAM".to_string(),
        TERMY_TERM_PROGRAM.to_string(),
    );

    for (key, value) in &runtime_config.environment {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        env_overrides.insert(key.to_string(), value.clone());
    }

    // Locale overrides are intentionally Unix-only. POSIX shells use libc locale
    // (`LC_*`/`LANG`) for wcwidth/prompt width, while native Windows shells
    // (`cmd.exe`/PowerShell) do not use this locale contract.
    #[cfg(unix)]
    {
        apply_utf8_locale_overrides(&mut env_overrides);
    }

    let shell_integration_enabled = shell_integration.is_some_and(|cfg| cfg.enabled);
    env_overrides.insert(
        "TERMY_SHELL_INTEGRATION".to_string(),
        if shell_integration_enabled { "1" } else { "0" }.to_string(),
    );

    if shell_integration_enabled {
        let prefix = shell_integration
            .and_then(|cfg| {
                let trimmed = cfg.explicit_prefix.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .unwrap_or("termy:tab:");
        env_overrides.insert("TERMY_TAB_TITLE_PREFIX".to_string(), prefix.to_string());
    }

    env_overrides
}

#[cfg(unix)]
fn apply_utf8_locale_overrides(env_overrides: &mut HashMap<String, String>) {
    let lc_all = env::var("LC_ALL").ok();
    let lc_ctype = env::var("LC_CTYPE").ok();
    let lang = env::var("LANG").ok();
    let target_utf8_locale =
        preferred_utf8_locale(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref());

    // zsh prompt width calculations rely on libc wcwidth + locale. If the shell
    // starts in C/POSIX/non-UTF-8 locale, multibyte prompt glyphs (e.g. U+276F)
    // can be counted by byte-length, drifting completion rendering.
    match utf8_locale_override_plan(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref()) {
        Utf8LocaleOverridePlan::None => {}
        Utf8LocaleOverridePlan::LcCtypeOnly => {
            env_overrides.insert("LC_CTYPE".to_string(), target_utf8_locale);
        }
        Utf8LocaleOverridePlan::LcAllAndLcCtype => {
            env_overrides.insert("LC_ALL".to_string(), target_utf8_locale.clone());
            env_overrides.insert("LC_CTYPE".to_string(), target_utf8_locale);
        }
    }
}

pub fn resolve_working_directory_path(configured: Option<&str>) -> Option<std::path::PathBuf> {
    let configured = configured?.trim();
    if configured.is_empty() {
        return None;
    }

    let path = if configured == "~" {
        user_home_dir()?
    } else if let Some(relative) = configured
        .strip_prefix("~/")
        .or_else(|| configured.strip_prefix("~\\"))
    {
        user_home_dir()?.join(relative)
    } else {
        PathBuf::from(configured)
    };

    if path.is_dir() { Some(path) } else { None }
}

pub fn resolve_launch_working_directory(
    configured: Option<&str>,
    fallback: WorkingDirFallback,
) -> Option<PathBuf> {
    resolve_working_directory_path(configured)
        .or_else(|| default_working_directory_with_fallback(fallback))
}

pub fn normalize_working_directory_candidate(candidate: Option<&str>) -> Option<String> {
    let candidate = candidate?.trim();
    if candidate.is_empty() || candidate.bytes().any(|byte| byte.is_ascii_control()) {
        return None;
    }

    Some(resolve_working_directory_path(Some(candidate)).map_or_else(
        || candidate.to_string(),
        |path| path.to_string_lossy().into_owned(),
    ))
}

fn default_working_directory_with_fallback(fallback: WorkingDirFallback) -> Option<PathBuf> {
    if fallback == WorkingDirFallback::Home
        && let Some(home) = user_home_dir()
        && home.is_dir()
    {
        return Some(home);
    }

    env::current_dir().ok()
}

/// Events sent from the terminal to the view
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    /// Terminal content has changed, needs redraw
    Wakeup,
    /// Terminal title changed
    #[allow(dead_code)]
    Title(String),
    /// Terminal title reset
    ResetTitle,
    /// Bell character received
    Bell,
    /// Terminal exited
    Exit,
    /// OSC 52 clipboard store request
    ClipboardStore(String),

    // Shell integration events (OSC 133)
    /// OSC 133;A - Shell prompt start
    ShellPromptStart,
    /// OSC 133;B - Command input start
    ShellCommandStart,
    /// OSC 133;C - Command executing
    ShellCommandExecuting,
    /// OSC 133;D - Command finished with optional exit code
    ShellCommandFinished(Option<i32>),

    // Progress indicator (OSC 9;4)
    /// Progress state change from OSC 9;4
    Progress(ProgressState),

    // Working directory (OSC 7)
    /// Working directory changed
    WorkingDirectory(String),
}

/// Host-provided callback used to schedule terminal event draining.
///
/// The callback is intentionally payload-free: hosts that multiplex several
/// terminals can capture their own stable terminal identifier, while the FFI
/// host can keep using its existing one-terminal wake channel.
#[derive(Clone)]
pub struct TerminalWakeupNotifier {
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl TerminalWakeupNotifier {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            notify: Arc::new(notify),
        }
    }

    fn notify(&self) {
        (self.notify)();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalDirtySpan {
    pub row: usize,
    pub left_col: usize,
    pub right_col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalDamageSnapshot {
    Full,
    Partial(Vec<TerminalDirtySpan>),
}

/// Terminal dimensions in cells and pixels
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }
}

impl TerminalSize {
    /// Clamp the cell dimensions into the supported range. Applied at every
    /// entry point that sizes the grid (`Terminal::new`, `new_display`,
    /// `resize`) so a buggy or hostile embedder cannot request a grid large
    /// enough to exhaust memory. The pixel cell metrics are left untouched.
    /// Columns/rows are floored at 1 so downstream grid math never sees a zero
    /// dimension.
    fn clamped(self) -> Self {
        Self {
            cols: self.cols.clamp(1, MAX_TERMINAL_COLS),
            rows: self.rows.clamp(1, MAX_TERMINAL_ROWS),
            ..self
        }
    }
}

/// The terminal state wrapper
pub struct Terminal {
    backend: engine_backend::Backend,
    engine_diagnostics: TerminalEngineDiagnostics,
}

mod engine_backend;
mod tmon_backend;

impl Terminal {
    /// The active engine name for diagnostics. Do not branch application
    /// behavior on this value; construction policy remains owned by core.
    pub fn engine_label(&self) -> &'static str {
        self.engine_diagnostics.engine
    }

    /// The actual engine and the private selector's construction reason.
    pub fn engine_diagnostics(&self) -> &TerminalEngineDiagnostics {
        &self.engine_diagnostics
    }

    /// Create a new terminal with the given size.
    pub fn new(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        event_wakeup_tx: Option<Sender<()>>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::select_native(
            size,
            configured_working_dir,
            event_wakeup_tx,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
        .map(Self::from_backend_selection)
    }

    /// Create a terminal whose wakeups are routed through a host callback.
    pub fn new_with_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new_with_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
        .map(Self::from_backend_selection)
    }

    /// Create a terminal whose child is selected with a typed launch contract.
    pub fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch,
        )
        .map(Self::from_backend_selection)
    }

    /// Create a display-only terminal with no PTY or child process.
    pub fn new_display(size: TerminalSize, runtime_config: Option<&TerminalRuntimeConfig>) -> Self {
        Self::from_backend_selection(engine_backend::Backend::new_display(size, runtime_config))
    }

    /// Create a display-only terminal whose committed output wakes the host.
    pub fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        Self::from_backend_selection(engine_backend::Backend::new_display_with_wakeup_notifier(
            size,
            runtime_config,
            wakeup_notifier,
        ))
    }

    fn from_backend_selection(selection: engine_backend::BackendSelection) -> Self {
        Self {
            backend: selection.backend,
            engine_diagnostics: selection.diagnostics,
        }
    }

    pub fn feed_output(&self, bytes: &[u8]) {
        self.backend.feed_output(bytes);
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.backend.child_pid()
    }

    pub fn set_wakeup_enabled(&self, enabled: bool) {
        self.backend.set_wakeup_enabled(enabled);
    }

    pub fn write(&self, input: &[u8]) {
        self.backend.write(input);
    }

    /// Try to enqueue bytes for the child PTY.
    ///
    /// Native backends report definite enqueue failures; Tmon also reports
    /// bounded-backlog failures. A display-only terminal accepts the write as
    /// a no-op.
    pub fn try_write(&self, input: &[u8]) -> io::Result<()> {
        self.backend.try_write(input)
    }

    pub fn write_owned(&self, input: Vec<u8>) {
        self.backend.write_owned(input);
    }

    /// Try to enqueue owned bytes for the child PTY without an extra copy.
    pub fn try_write_owned(&self, input: Vec<u8>) -> io::Result<()> {
        self.backend.try_write_owned(input)
    }

    pub fn hydrate_output(&self, bytes: &[u8]) {
        self.backend.hydrate_output(bytes);
    }

    #[allow(dead_code)]
    pub fn write_str(&self, input: &str) {
        self.backend.write_str(input);
    }

    pub fn try_write_str(&self, input: &str) -> io::Result<()> {
        self.backend.try_write_str(input)
    }

    pub fn resize(&mut self, new_size: TerminalSize) {
        self.backend.resize(new_size);
    }

    /// Resize the child PTY and grid, returning any PTY enqueue failure.
    pub fn try_resize(&mut self, new_size: TerminalSize) -> io::Result<()> {
        self.backend.try_resize(new_size)
    }

    pub fn nudge_resize(&self) {
        self.backend.nudge_resize();
    }

    /// Re-send the current size and return any PTY enqueue failure.
    pub fn try_nudge_resize(&self) -> io::Result<()> {
        self.backend.try_nudge_resize()
    }

    pub fn size(&self) -> TerminalSize {
        self.backend.size()
    }

    pub fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.backend.kitty_graphics_placements()
    }

    pub fn kitty_graphics_revision(&self) -> u64 {
        self.backend.kitty_graphics_revision()
    }

    pub fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        self.backend.kitty_graphics_snapshot()
    }

    pub fn drain_events(&self, host: &mut impl TerminalReplyHost) -> (Vec<TerminalEvent>, bool) {
        self.backend.drain_events(host)
    }

    pub fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        self.backend.set_query_colors(query_colors);
    }

    pub fn palette(&self) -> crate::TerminalPalette {
        self.backend.palette()
    }

    pub fn snapshot(&self) -> TermyFrame {
        self.backend.snapshot()
    }

    pub fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        self.backend.frame_update(force_full)
    }

    pub fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        self.backend.take_render_damage_snapshot()
    }

    pub fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        self.backend.render_read(force_full)
    }

    pub fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        self.backend.visit_viewport_cells(visitor)
    }

    pub fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        self.backend
            .visit_viewport_ranges_at_generation(generation, spans, visitor)
    }

    pub fn line_bounds(&self) -> (i32, i32) {
        self.backend.line_bounds()
    }

    /// Visit a requested inclusive buffer-line range from one coherent backend state.
    ///
    /// The callback runs under the backend lock and must not call back into this terminal.
    pub fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        visitor: impl FnMut((i32, i32, usize), i32, usize, &crate::TerminalRenderCell),
    ) -> (i32, i32, usize) {
        self.backend
            .visit_line_cells(requested_first, requested_last, visitor)
    }

    pub fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        self.backend.search(query)
    }

    pub fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        self.backend.search_with_options(query, options)
    }

    pub fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        self.backend.search_shared(query)
    }

    pub fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        self.backend.search_shared_with_options(query, options)
    }

    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<crate::links::DetectedLink> {
        self.backend.hyperlink_at(row, col)
    }

    pub fn link_at(&self, row: usize, col: usize) -> Option<crate::links::DetectedViewportLink> {
        self.backend.link_at(row, col)
    }

    pub fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        self.backend.take_damage_snapshot()
    }

    pub fn scroll_display(&self, delta_lines: i32) -> bool {
        self.backend.scroll_display(delta_lines)
    }

    pub fn scroll_to_bottom(&self) -> bool {
        self.backend.scroll_to_bottom()
    }

    pub fn clear_scrollback(&self) -> bool {
        self.backend.clear_scrollback()
    }

    pub fn scroll_state(&self) -> (usize, usize) {
        self.backend.scroll_state()
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.backend.cursor_state()
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        self.backend.cursor_position()
    }

    #[allow(dead_code)]
    pub fn has_pending_events(&self) -> bool {
        self.backend.has_pending_events()
    }

    pub fn set_term_options(&self, options: TerminalOptions) {
        self.backend.set_term_options(options);
    }

    pub fn set_scrollback_history(&self, scrollback_history: usize) {
        self.backend.set_scrollback_history(scrollback_history);
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.backend.bracketed_paste_mode()
    }

    pub fn mouse_mode(&self) -> TerminalMouseMode {
        self.backend.mouse_mode()
    }

    pub fn keyboard_mode(&self) -> TerminalKeyboardMode {
        self.backend.keyboard_mode()
    }

    pub fn alternate_screen_mode(&self) -> bool {
        self.backend.alternate_screen_mode()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_TERM, GHOSTTY_COMPAT_TERM_PROGRAM, GHOSTTY_COMPAT_TERM_PROGRAM_VERSION,
        MAX_TERMINAL_COLS, MAX_TERMINAL_ROWS, MAX_TERMINAL_SCROLLBACK_HISTORY, TERMY_TERM_PROGRAM,
        Terminal, TerminalCursorState, TerminalCursorStyle, TerminalDamageSnapshot, TerminalLaunch,
        TerminalOptions, TerminalRuntimeConfig, TerminalSize, TerminalWakeupNotifier, WindowsShell,
        WorkingDirFallback, normalize_working_directory_candidate,
        resolve_launch_working_directory, resolve_shell_path, resolve_terminal_launch,
        terminal_environment_overrides, user_home_dir,
    };
    #[cfg(target_os = "windows")]
    use super::{default_shell_launch, windows_cmd_path, windows_git_bash_path};
    use crate::keyboard::{
        Keystroke, Modifiers, TerminalKeyEventKind, TerminalKeyboardMode, keystroke_to_input,
    };
    use crate::protocol::{TerminalClipboardTarget, TerminalReplyHost};
    use std::collections::HashMap;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use termy_terminal_test_support::{
        CLAUDE_CODE_2_1_233_CURSOR_CELL, CLAUDE_CODE_2_1_233_INITIAL_FRAME,
        NEOVIM_0_12_4_CURSOR_CELL, NEOVIM_0_12_4_RUST_FRAME, TerminalTrace,
    };

    fn replay_real_tui_trace(trace: TerminalTrace, bytes: &[u8], chunk_size: usize) -> Terminal {
        let terminal = Terminal::new_display(
            TerminalSize {
                cols: trace.cols,
                rows: trace.rows,
                ..TerminalSize::default()
            },
            None,
        );
        for chunk in bytes.chunks(chunk_size) {
            terminal.feed_output(chunk);
        }
        terminal
    }

    fn assert_real_tui_trace_is_chunk_invariant(
        trace: TerminalTrace,
        chunk_sizes: &[usize],
    ) -> Terminal {
        let bytes = trace.bytes();
        let baseline = replay_real_tui_trace(trace, &bytes, bytes.len());
        let expected_cells = baseline.render_read(true).cells;
        let expected_cursor = baseline.cursor_state();
        let expected_mouse = baseline.mouse_mode();
        let expected_keyboard = baseline.keyboard_mode();
        let expected_scrolling = baseline.scroll_state();

        for &chunk_size in chunk_sizes {
            let terminal = replay_real_tui_trace(trace, &bytes, chunk_size);
            assert_eq!(
                terminal.render_read(true).cells,
                expected_cells,
                "{} chunk size {chunk_size}",
                trace.id
            );
            assert_eq!(terminal.cursor_state(), expected_cursor);
            assert_eq!(terminal.mouse_mode(), expected_mouse);
            assert_eq!(terminal.keyboard_mode(), expected_keyboard);
            assert_eq!(terminal.scroll_state(), expected_scrolling);
            assert_eq!(
                terminal.alternate_screen_mode(),
                baseline.alternate_screen_mode()
            );
            assert_eq!(
                terminal.bracketed_paste_mode(),
                baseline.bracketed_paste_mode()
            );
        }

        baseline
    }

    #[test]
    fn terminal_size_clamps_absurd_dimensions() {
        let huge = TerminalSize {
            cols: u16::MAX,
            rows: u16::MAX,
            cell_width: 8.0,
            cell_height: 16.0,
        }
        .clamped();
        assert_eq!(huge.cols, MAX_TERMINAL_COLS);
        assert_eq!(huge.rows, MAX_TERMINAL_ROWS);
    }

    #[test]
    fn display_terminal_intercepts_and_places_kitty_graphics() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let initial_revision = terminal.kitty_graphics_revision();
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=77,c=2,r=3;AQID/w==\x1b\\");

        let (revision, placements) = terminal.kitty_graphics_snapshot();
        assert!(revision > initial_revision);
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 77);
        assert_eq!(placements[0].display_cols, Some(2));
        assert_eq!(placements[0].display_rows, Some(3));
        assert!(placements[0].png.starts_with(b"\x89PNG"));

        let cursor = terminal.cursor_position();
        assert_eq!(cursor, (2, 3));
    }

    #[test]
    fn real_tui_claude_code_trace_is_chunk_invariant() {
        let trace = CLAUDE_CODE_2_1_233_INITIAL_FRAME;
        let terminal = assert_real_tui_trace_is_chunk_invariant(trace, &[1, 7, 64, 257]);
        let cells = terminal.render_read(true).cells;
        let (expected_row, expected_col) = CLAUDE_CODE_2_1_233_CURSOR_CELL;
        let cursor_cell = &cells[expected_row * usize::from(trace.cols) + expected_col];
        assert_eq!(cursor_cell.text, " ");
        assert_eq!(
            cursor_cell.foreground,
            crate::TerminalRenderColor::DefaultForeground
        );
        assert_eq!(
            cursor_cell.background,
            crate::TerminalRenderColor::DefaultBackground
        );
        assert!(cursor_cell.inverse);
        assert_eq!(terminal.cursor_state(), None);
        assert!(terminal.alternate_screen_mode());
    }

    #[test]
    fn real_tui_neovim_trace_preserves_rich_render_cells_and_modes() {
        let trace = NEOVIM_0_12_4_RUST_FRAME;
        let terminal = assert_real_tui_trace_is_chunk_invariant(trace, &[1, 7, 64, 257, 1024]);
        let cells = terminal.render_read(true).cells;
        let index = |row, col| row * usize::from(trace.cols) + col;
        let (cursor_row, cursor_col) = NEOVIM_0_12_4_CURSOR_CELL;

        assert_eq!(
            terminal.cursor_state(),
            Some(TerminalCursorState {
                col: cursor_col,
                row: cursor_row,
                style: TerminalCursorStyle::Block,
            })
        );
        assert!(terminal.alternate_screen_mode());
        assert!(terminal.bracketed_paste_mode());
        let mouse = terminal.mouse_mode();
        assert!(mouse.enabled);
        assert!(mouse.report_drag);
        assert!(mouse.sgr_encoding);

        let cursor_cell = &cells[index(cursor_row, cursor_col)];
        assert_eq!(cursor_cell.text, "T");
        assert_eq!(
            cursor_cell.foreground,
            crate::TerminalRenderColor::Rgb(crate::TerminalColor {
                r: 31,
                g: 36,
                b: 48
            })
        );
        assert_eq!(
            cursor_cell.background,
            crate::TerminalRenderColor::Rgb(crate::TerminalColor {
                r: 246,
                g: 193,
                b: 119,
            })
        );
        assert!(cursor_cell.bold);

        assert_eq!(cells[index(1, 26)].text, "✓");
        let wide = &cells[index(1, 28)];
        let spacer = &cells[index(1, 29)];
        assert_eq!(wide.text, "界");
        assert_eq!(wide.underline_style, crate::TerminalUnderlineStyle::Curly);
        assert_eq!(
            wide.underline_color,
            Some(crate::TerminalRenderColor::Rgb(crate::TerminalColor {
                r: 246,
                g: 193,
                b: 119,
            }))
        );
        assert!(spacer.wide_character_spacer);
        assert_eq!(spacer.underline_style, crate::TerminalUnderlineStyle::Curly);
    }

    #[test]
    fn display_terminal_notifier_coalesces_until_events_are_drained() {
        let notifications = Arc::new(AtomicU64::new(0));
        let notification_count = notifications.clone();
        let terminal = Terminal::new_display_with_wakeup_notifier(
            test_terminal_size(),
            None,
            Some(TerminalWakeupNotifier::new(move || {
                notification_count.fetch_add(1, Ordering::Relaxed);
            })),
        );

        terminal.feed_output(b"a");
        terminal.feed_output(b"b");
        assert_eq!(notifications.load(Ordering::Relaxed), 1);

        let mut reply_host = RecordingReplyHost::default();
        let _ = terminal.drain_events(&mut reply_host);
        terminal.feed_output(b"c");
        assert_eq!(notifications.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn kitty_command_only_cursor_movement_rejects_old_render_generation() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let update = terminal.take_render_damage_snapshot();

        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=177,c=2,r=3;AQID/w==\x1b\\");

        let mut visited = 0;
        assert!(!terminal.visit_viewport_ranges_at_generation(
            update.generation,
            &[crate::TerminalDirtySpan {
                row: 0,
                left_col: 0,
                right_col: 0,
            }],
            |_, _, _, _, _| visited += 1,
        ));
        assert_eq!(visited, 0);
    }

    #[test]
    fn display_terminal_scrolls_for_kitty_cursor_advance_at_bottom() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=78,c=2,r=3;AQID/w==\x1b\\");

        assert_eq!(terminal.scroll_state(), (0, 3));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_tracks_zero_history_screen_scroll() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 0,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=79,c=2,r=3;AQID/w==\x1b\\");

        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_tracks_full_history_screen_scroll() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 2,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[4;1H\n\n");
        assert_eq!(terminal.scroll_state(), (0, 2));

        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=81,c=2,r=3;AQID/w==\x1b\\");

        assert_eq!(terminal.scroll_state(), (0, 2));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_tracks_alternate_screen_scroll() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal
            .feed_output(b"\x1b[?1049h\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=80,c=2,r=3;AQID/w==\x1b\\");

        assert!(terminal.alternate_screen_mode());
        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn ordinary_newlines_shift_and_remove_alternate_screen_kitty_placement() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=86,c=2,r=1,C=1;AQID/w==\x1b\\",
        );
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);

        terminal.feed_output(b"\n");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn synchronized_newlines_shift_alternate_screen_kitty_placement() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=96,c=2,r=1,C=1;AQID/w==\x1b\\",
        );

        terminal.feed_output(b"\x1b[?2026h\x1b[3;1H\n\n\x1b[?2026l");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn ordinary_newlines_shift_and_remove_zero_history_kitty_placement() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 0,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=87,c=2,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);

        terminal.feed_output(b"\n");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn ordinary_wrapped_text_shifts_and_removes_zero_history_kitty_placement() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 0,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=97,c=2,r=1,C=1;AQID/w==\x1b\\");

        terminal.feed_output(b"\x1b[4;32Hab");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);

        terminal.feed_output("\x1b[4;32H界".as_bytes());
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn ordinary_newlines_shift_and_remove_full_history_kitty_placement() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 1,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.scroll_state(), (0, 1));
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=88,c=2,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[4;1H\n");
        assert_eq!(terminal.scroll_state(), (0, 1));
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 0);

        terminal.feed_output(b"\n");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn ordinary_partial_region_scroll_does_not_shift_kitty_placement() {
        let runtime_config = TerminalRuntimeConfig {
            scrollback_history: 0,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(b"\x1b[1;1H\x1b_Ga=T,f=32,s=1,v=1,i=89,c=2,r=1,C=1;AQID/w==\x1b\\");

        terminal.feed_output(b"\x1b[2;3r\x1b[3;1H\n");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn top_anchored_partial_region_scroll_keeps_footer_kitty_placement_fixed() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=90,c=2,r=1,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements()[0].viewport_row, 3);

        terminal.feed_output(b"\x1b[1;3r\x1b[3;1H\n");

        assert_eq!(terminal.scroll_state(), (0, 1));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 3);
    }

    #[test]
    fn kitty_cursor_advance_does_not_scroll_partial_decstbm_region() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(
            b"\x1b[2;3r\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=82,c=2,r=3;AQID/w==\x1b\\\x1b[r",
        );

        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (0, 0));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 82);
        assert_eq!(placements[0].viewport_row, 2);
    }

    #[test]
    fn primary_kitty_placement_survives_alternate_screen_scroll() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=83,c=2,r=2,C=1;AQID/w==\x1b\\");
        let primary = terminal.kitty_graphics_placements();
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].image_id, 83);
        assert_eq!(primary[0].viewport_row, 1);

        terminal
            .feed_output(b"\x1b[?1049h\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=84,c=2,r=3;AQID/w==\x1b\\");
        let alternate = terminal.kitty_graphics_placements();
        assert_eq!(alternate.len(), 1);
        assert_eq!(alternate[0].image_id, 84);
        assert_eq!(alternate[0].viewport_row, 0);

        terminal.feed_output(b"\x1b[?1049l");
        let restored_primary = terminal.kitty_graphics_placements();
        assert_eq!(restored_primary.len(), 1);
        assert_eq!(restored_primary[0].image_id, 83);
        assert_eq!(restored_primary[0].viewport_row, 1);

        terminal.feed_output(b"\x1b[?1049h");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn terminal_reset_clears_kitty_graphics() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=85,c=2,r=2,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);

        terminal.feed_output(b"\x1bc");

        assert!(!terminal.alternate_screen_mode());
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn clear_screen_erases_only_the_active_viewport_graphics() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=98,c=2,r=1,C=1;AQID/w==\x1b\\");

        terminal.feed_output(b"\x1b[J\x1b[1J\x1b[3J");
        assert_eq!(
            terminal.kitty_graphics_placements().len(),
            1,
            "non-ED2 erase commands must not affect graphics"
        );

        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=99,c=2,r=1,C=1;AQID/w==\x1b\\",
        );
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);
        terminal.feed_output(b"\x1b[2J");
        assert!(terminal.kitty_graphics_placements().is_empty());

        terminal.feed_output(b"\x1b[?1049l");
        assert_eq!(
            terminal.kitty_graphics_placements().len(),
            1,
            "clearing the alternate viewport must preserve primary graphics"
        );

        terminal.feed_output(b"\x1b[H\x1b[2J");
        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn terminal_size_clamp_leaves_realistic_dimensions_untouched() {
        let clamped = TerminalSize {
            cols: 200,
            rows: 60,
            cell_width: 9.0,
            cell_height: 18.0,
        }
        .clamped();
        assert_eq!(clamped.cols, 200);
        assert_eq!(clamped.rows, 60);
    }

    #[test]
    fn identical_terminal_resize_does_not_redamage_the_grid() {
        let size = TerminalSize {
            cols: 80,
            rows: 24,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut terminal = Terminal::new_display(size, None);
        assert_eq!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Full
        );
        let stable_damage = terminal.take_damage_snapshot();
        assert_eq!(terminal.take_damage_snapshot(), stable_damage);

        terminal.resize(size);

        assert_eq!(terminal.size(), size);
        assert_eq!(terminal.take_damage_snapshot(), stable_damage);
    }

    #[test]
    fn terminal_size_clamp_floors_zero_dimensions_at_one() {
        let empty = TerminalSize {
            cols: 0,
            rows: 0,
            cell_width: 9.0,
            cell_height: 18.0,
        }
        .clamped();
        assert_eq!(empty.cols, 1);
        assert_eq!(empty.rows, 1);
    }

    #[test]
    fn terminal_options_clamp_scrollback_history() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.set_term_options(TerminalOptions {
            scrollback_history: 10_000_000,
            default_cursor_style: TerminalCursorStyle::Block,
        });
        terminal.feed_output(
            &(0..MAX_TERMINAL_SCROLLBACK_HISTORY.saturating_add(100))
                .map(|_| "x\r\n")
                .collect::<String>()
                .into_bytes(),
        );
        assert!(terminal.scroll_state().1 <= MAX_TERMINAL_SCROLLBACK_HISTORY);
    }

    #[test]
    fn terminal_options_preserve_in_range_scrollback_history() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.set_term_options(TerminalOptions {
            scrollback_history: 5,
            default_cursor_style: TerminalCursorStyle::Block,
        });
        terminal.feed_output(&(0..20).map(|_| "x\r\n").collect::<String>().into_bytes());
        assert_eq!(terminal.scroll_state().1, 5);
    }

    fn test_terminal_size() -> TerminalSize {
        TerminalSize {
            cols: 32,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        }
    }

    fn cursor_after_bytes(input: &[u8]) -> (usize, usize) {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(input);
        terminal.cursor_position()
    }

    #[test]
    fn search_term_buffer_includes_scrollback_rows() {
        let size = TerminalSize {
            cols: 16,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let config = TerminalRuntimeConfig {
            scrollback_history: 8,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(size, Some(&config));
        terminal.feed_output(b"alpha\r\nbeta\r\ngamma");

        let matches = terminal.search("alpha");

        assert_eq!(terminal.scroll_state().1, 1);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].row, 0);
        assert_eq!(matches[0].start_col, 0);
        assert_eq!(matches[0].end_col, 4);
    }

    fn cursor_state_after_bytes(
        input: &[u8],
        runtime_config: TerminalRuntimeConfig,
    ) -> Option<TerminalCursorState> {
        let terminal = Terminal::new_display(test_terminal_size(), Some(&runtime_config));
        terminal.feed_output(input);
        terminal.cursor_state()
    }

    fn cursor_position_after_bytes(input: &[u8]) -> (usize, usize) {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(input);
        terminal.cursor_position()
    }

    fn mouse_mode_after_bytes(input: &[u8]) -> crate::mouse_protocol::TerminalMouseMode {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(input);
        terminal.mouse_mode()
    }

    fn keystroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_string(),
            key_char: None,
        }
    }

    fn press_mode() -> TerminalKeyboardMode {
        TerminalKeyboardMode::default()
    }

    fn keyboard_mode_after_bytes(input: &[u8]) -> TerminalKeyboardMode {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(input);
        terminal.keyboard_mode()
    }

    #[derive(Default)]
    struct RecordingReplyHost {
        clipboard_text: Option<String>,
        requested_targets: Vec<TerminalClipboardTarget>,
    }

    impl TerminalReplyHost for RecordingReplyHost {
        fn load_clipboard(&mut self, target: TerminalClipboardTarget) -> Option<String> {
            self.requested_targets.push(target);
            self.clipboard_text.clone()
        }
    }

    #[test]
    fn normalize_working_directory_candidate_preserves_relative_paths() {
        assert_eq!(
            normalize_working_directory_candidate(Some(" crates/cli ")).as_deref(),
            Some("crates/cli")
        );
    }

    #[test]
    fn normalize_working_directory_candidate_rejects_control_characters() {
        assert_eq!(
            normalize_working_directory_candidate(Some("/tmp/project\nrun-shell")),
            None
        );
    }

    #[test]
    fn resolve_launch_working_directory_falls_back_when_configured_path_is_invalid() {
        let fallback = std::env::current_dir().expect("current dir");
        let resolved = resolve_launch_working_directory(
            Some("/definitely/not/a/real/termy/path"),
            WorkingDirFallback::Process,
        )
        .expect("fallback path");
        assert_eq!(resolved, fallback);
    }

    #[test]
    fn normalize_working_directory_candidate_expands_home_directory() {
        let expected = user_home_dir()
            .expect("home dir")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            normalize_working_directory_candidate(Some("~")).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn mouse_mode_detects_click_reporting() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1000h");
        assert!(mode.enabled);
        assert!(mode.report_click);
        assert!(!mode.report_drag);
        assert!(!mode.report_motion);
    }

    #[test]
    fn mouse_mode_detects_drag_reporting() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1002h");
        assert!(mode.enabled);
        assert!(mode.report_drag);
        assert!(!mode.report_motion);
    }

    #[test]
    fn mouse_mode_detects_motion_reporting() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1003h");
        assert!(mode.enabled);
        assert!(mode.report_motion);
    }

    #[test]
    fn mouse_mode_detects_sgr_encoding() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1006h");
        assert!(mode.sgr_encoding);
    }

    #[test]
    fn mouse_mode_detects_utf8_reporting() {
        let mode = mouse_mode_after_bytes(b"\x1b[?1005h");
        assert!(mode.utf8_encoding);
    }

    #[test]
    fn terminal_damage_snapshot_is_full_for_new_terminal() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        assert!(matches!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Full
        ));
    }

    #[test]
    fn terminal_damage_snapshot_resets_damage_after_read() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        let second = terminal.take_damage_snapshot();
        let third = terminal.take_damage_snapshot();
        assert!(matches!(second, TerminalDamageSnapshot::Partial(_)));
        assert_eq!(second, third);
    }

    #[test]
    fn terminal_damage_snapshot_returns_partial_spans_for_output() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        terminal.feed_output(b"abc");
        assert!(matches!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Partial(spans) if !spans.is_empty()
        ));
    }

    #[test]
    fn terminal_damage_snapshot_while_scrolled_returns_empty_partial_without_damage() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
        let _ = terminal.take_damage_snapshot();

        assert!(terminal.scroll_display(1));
        assert!(terminal.scroll_state().0 > 0);

        assert!(matches!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Full
        ));
        assert_eq!(
            terminal.take_damage_snapshot(),
            TerminalDamageSnapshot::Partial(Vec::new())
        );
    }

    #[test]
    fn terminal_damage_snapshot_while_scrolled_maps_damage_to_viewport_rows() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
        let _ = terminal.take_damage_snapshot();

        assert!(terminal.scroll_display(1));
        let _ = terminal.take_damage_snapshot();
        let _ = terminal.take_damage_snapshot();

        terminal.feed_output(b"\x1b[1;1H");
        match terminal.take_damage_snapshot() {
            TerminalDamageSnapshot::Partial(spans) => {
                assert!(spans.iter().any(|span| span.row == 1), "spans: {spans:?}");
                assert!(spans.iter().all(|span| span.row < 4), "spans: {spans:?}");
            }
            TerminalDamageSnapshot::Full => {
                panic!("visible damage while scrolled should stay partial")
            }
        }
    }

    #[test]
    fn terminal_damage_snapshot_while_scrolled_drops_damage_below_viewport() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        let _ = terminal.take_damage_snapshot();
        terminal.feed_output(b"1\n2\n3\n4\n5\n6\n");
        let _ = terminal.take_damage_snapshot();

        assert!(terminal.scroll_display(3));
        let _ = terminal.take_damage_snapshot();
        let _ = terminal.take_damage_snapshot();

        terminal.feed_output(b"x");
        match terminal.take_damage_snapshot() {
            TerminalDamageSnapshot::Partial(spans) => {
                assert!(spans.iter().all(|span| span.row < 4), "spans: {spans:?}");
            }
            TerminalDamageSnapshot::Full => {
                panic!("invisible damage while scrolled should stay partial")
            }
        }
    }

    #[test]
    fn rich_render_read_preserves_text_colors_attributes_and_metadata() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(
            "\x1b[38;2;1;2;3;48;5;4;4:3;58:2::7:8:9;1;2;3;7;8;9me\u{301}\x1b[0m 界".as_bytes(),
        );

        let read = terminal.render_read(true);
        assert_eq!((read.metadata.cols, read.metadata.rows), (32, 4));
        assert_eq!(read.metadata.generation, read.update.generation);
        assert_eq!(read.metadata.palette_revision, read.update.palette_revision);
        assert_eq!(read.metadata.palette_revision, read.palette.revision);
        assert!(matches!(read.update.damage, TerminalDamageSnapshot::Full));
        assert!(read.update.scrolls.is_empty());

        let cell = &read.cells[0];
        assert_eq!(cell.text, "e\u{301}");
        assert_eq!(
            cell.foreground,
            crate::TerminalRenderColor::Rgb(crate::TerminalColor { r: 1, g: 2, b: 3 })
        );
        assert_eq!(cell.background, crate::TerminalRenderColor::Indexed(4));
        assert_eq!(
            cell.underline_color,
            Some(crate::TerminalRenderColor::Rgb(crate::TerminalColor {
                r: 7,
                g: 8,
                b: 9,
            }))
        );
        assert_eq!(cell.underline_style, crate::TerminalUnderlineStyle::Curly);
        assert!(cell.bold);
        assert!(cell.dim);
        assert!(cell.italic);
        assert!(cell.inverse);
        assert!(cell.hidden);
        assert!(cell.strikethrough);
        assert!(read.cells.iter().any(|cell| cell.wide_character_spacer));
    }

    #[test]
    fn viewport_visitors_allow_reentrant_terminal_reads() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"abc");

        let mut viewport_reentered = false;
        terminal.visit_viewport_cells(|_, _, _, _| {
            if !viewport_reentered {
                viewport_reentered = true;
                assert_eq!(terminal.line_bounds().1, 3);
            }
        });
        assert!(viewport_reentered);

        let update = terminal.take_render_damage_snapshot();
        let spans = match &update.damage {
            TerminalDamageSnapshot::Partial(spans) if !spans.is_empty() => spans.clone(),
            TerminalDamageSnapshot::Full | TerminalDamageSnapshot::Partial(_) => {
                vec![crate::TerminalDirtySpan {
                    row: 0,
                    left_col: 0,
                    right_col: 0,
                }]
            }
        };
        let mut range_reentered = false;
        assert!(terminal.visit_viewport_ranges_at_generation(
            update.generation,
            &spans,
            |_, _, _, _, _| {
                if !range_reentered {
                    range_reentered = true;
                    let _ = terminal.palette();
                }
            },
        ));
        assert!(range_reentered);
    }

    #[test]
    fn rich_line_visitor_streams_cells_in_buffer_order() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"abc\r\ndef");

        let mut visited = Vec::new();
        let bounds = terminal.visit_line_cells(0, 1, |range, line, col, cell| {
            if col < 3 {
                visited.push((range, line, col, cell.text.to_string()));
            }
        });

        assert_eq!(bounds, (0, 3, 32));
        assert_eq!(
            visited,
            vec![
                ((0, 3, 32), 0, 0, "a".to_string()),
                ((0, 3, 32), 0, 1, "b".to_string()),
                ((0, 3, 32), 0, 2, "c".to_string()),
                ((0, 3, 32), 1, 0, "d".to_string()),
                ((0, 3, 32), 1, 1, "e".to_string()),
                ((0, 3, 32), 1, 2, "f".to_string()),
            ]
        );
    }

    #[test]
    fn rich_render_cell_keeps_common_text_inline() {
        let terminal = Terminal::new_display(test_terminal_size(), None);
        terminal.feed_output(b"plain");

        let read = terminal.render_read(true);
        assert!(read.cells.iter().all(|cell| !cell.text.is_heap_allocated()));

        terminal.feed_output("\rcombined e\u{301}".as_bytes());
        let read = terminal.render_read(true);
        assert!(
            read.cells
                .iter()
                .filter(|cell| !cell.text.is_empty())
                .all(|cell| !cell.text.is_heap_allocated())
        );
    }

    #[test]
    fn rich_render_read_reports_wrap_generation_palette_and_partial_cells() {
        let terminal = Terminal::new_display(
            TerminalSize {
                cols: 4,
                rows: 2,
                ..test_terminal_size()
            },
            None,
        );
        let initial = terminal.render_read(true);
        let _ = terminal.take_render_damage_snapshot();
        terminal.feed_output(b"abcde");

        let update = terminal.take_render_damage_snapshot();
        assert!(update.generation > initial.metadata.generation);
        assert!(matches!(
            update.damage,
            TerminalDamageSnapshot::Partial(ref spans) if !spans.is_empty()
        ));
        let spans = match &update.damage {
            TerminalDamageSnapshot::Partial(spans) => spans,
            TerminalDamageSnapshot::Full => unreachable!(),
        };
        let mut visited = Vec::new();
        assert!(terminal.visit_viewport_ranges_at_generation(
            update.generation,
            spans,
            |row, display_offset, line, col, cell| {
                visited.push((row, display_offset, line, col, cell.text.clone()));
            },
        ));
        assert!(!visited.is_empty());

        terminal.feed_output(b"\x1b]4;1;#123456\x07");
        let read = terminal.render_read(true);
        assert!(read.cells[3].line_wrapped);
        assert_eq!(
            read.palette.indexed[1],
            Some(crate::TerminalColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            })
        );
        assert!(read.metadata.palette_revision > initial.metadata.palette_revision);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_secondary_shortcuts_map_to_line_editing_sequences() {
        let secondary = Modifiers {
            platform: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x01".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("home", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x01".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x05".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("end", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x05".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x15".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x0b".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_alt_shortcuts_map_to_word_editing_sequences() {
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", alt),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", alt),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bf".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", alt),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b\x7f".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", alt),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bd".to_vec())
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_secondary_shortcuts_map_to_native_word_sequences() {
        let secondary = Modifiers {
            control: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bf".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x17".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1bd".to_vec())
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_secondary_shortcuts_do_not_remap_in_alternate_screen() {
        let secondary = Modifiers {
            control: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("left", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                false,
            ),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                false,
            ),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                false,
            ),
            Some(vec![0x7f])
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", secondary),
                TerminalKeyEventKind::Press,
                press_mode(),
                false,
            ),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn plain_special_key_sequences_remain_unchanged() {
        let none = Modifiers::default();

        assert_eq!(
            keystroke_to_input(
                &keystroke("backspace", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(vec![0x7f])
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("delete", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("left", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[D".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("right", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("home", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("end", none),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(b"\x1b[F".to_vec())
        );
    }

    #[test]
    fn control_letter_mappings_remain_unchanged() {
        let control = Modifiers {
            control: true,
            ..Default::default()
        };

        assert_eq!(
            keystroke_to_input(
                &keystroke("a", control),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(vec![0x01])
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("c", control),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(vec![0x03])
        );
        assert_eq!(
            keystroke_to_input(
                &keystroke("z", control),
                TerminalKeyEventKind::Press,
                press_mode(),
                true,
            ),
            Some(vec![0x1a])
        );
    }

    #[test]
    fn keyboard_mode_detects_report_all_and_event_types() {
        let mode = keyboard_mode_after_bytes(b"\x1b[=10u");
        assert!(mode.report_all_keys_as_esc());
        assert!(mode.report_event_types());
        assert!(mode.enhanced_reporting_active());
    }

    #[test]
    fn keyboard_mode_augment_only_flags_do_not_activate_enhanced_reporting() {
        let mode = keyboard_mode_after_bytes(b"\x1b[=20u");
        assert!(mode.report_alternate_keys());
        assert!(mode.report_associated_text());
        assert!(!mode.enhanced_reporting_active());
    }

    #[test]
    fn env_overrides_set_term_by_default() {
        let env = terminal_environment_overrides(None, &TerminalRuntimeConfig::default());
        assert_eq!(env.get("TERM").map(String::as_str), Some(DEFAULT_TERM));
    }

    #[test]
    fn env_overrides_advertise_ghostty_progress_capability() {
        let env = terminal_environment_overrides(None, &TerminalRuntimeConfig::default());
        assert_eq!(
            env.get("TERM_PROGRAM").map(String::as_str),
            Some(GHOSTTY_COMPAT_TERM_PROGRAM)
        );
        assert_eq!(
            env.get("TERM_PROGRAM_VERSION").map(String::as_str),
            Some(GHOSTTY_COMPAT_TERM_PROGRAM_VERSION)
        );
        assert_eq!(
            env.get("TERMY_TERM_PROGRAM").map(String::as_str),
            Some(TERMY_TERM_PROGRAM)
        );
    }

    #[test]
    fn env_overrides_allow_disabling_colorterm() {
        let config = TerminalRuntimeConfig {
            colorterm: None,
            ..TerminalRuntimeConfig::default()
        };
        let env = terminal_environment_overrides(None, &config);
        assert!(!env.contains_key("COLORTERM"));
    }

    #[test]
    fn env_overrides_merge_host_environment_last() {
        let config = TerminalRuntimeConfig {
            environment: HashMap::from([
                ("CMUX_SOCKET_PATH".to_string(), "/tmp/cmux.sock".to_string()),
                ("TERM_PROGRAM".to_string(), "cmux".to_string()),
            ]),
            ..TerminalRuntimeConfig::default()
        };
        let env = terminal_environment_overrides(None, &config);
        assert_eq!(
            env.get("CMUX_SOCKET_PATH").map(String::as_str),
            Some("/tmp/cmux.sock")
        );
        assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("cmux"));
    }

    #[test]
    fn explicit_shell_path_wins() {
        assert_eq!(resolve_shell_path(Some("/bin/custom")), "/bin/custom");
        let config = TerminalRuntimeConfig {
            shell: Some("/bin/custom".to_string()),
            windows_shell: WindowsShell::PowerShell,
            ..TerminalRuntimeConfig::default()
        };
        let launch = resolve_terminal_launch(&config, None).expect("configured shell launch");
        assert_eq!(launch.program, "/bin/custom");
        assert_eq!(config.resolved_shell_program(), "/bin/custom");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn configured_unix_shell_keeps_macos_interactive_login_arguments() {
        let resolved = resolve_terminal_launch(
            &TerminalRuntimeConfig {
                shell: Some("/opt/custom/bin/zsh".to_string()),
                ..TerminalRuntimeConfig::default()
            },
            None,
        )
        .expect("configured shell launch");

        assert_eq!(resolved.program, "/opt/custom/bin/zsh");
        assert_eq!(resolved.args, ["-i", "-l"]);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn configured_unix_shell_keeps_non_macos_interactive_arguments() {
        let resolved = resolve_terminal_launch(
            &TerminalRuntimeConfig {
                shell: Some("/opt/custom/bin/zsh".to_string()),
                ..TerminalRuntimeConfig::default()
            },
            None,
        )
        .expect("configured shell launch");

        assert_eq!(resolved.program, "/opt/custom/bin/zsh");
        assert_eq!(resolved.args, ["-i"]);
    }

    #[test]
    fn typed_program_launch_keeps_arguments_out_of_the_shell() {
        let launch = TerminalLaunch::Program {
            program: "ssh".to_string(),
            args: vec![
                "-i".to_string(),
                "/tmp/key; touch /tmp/should-not-exist".to_string(),
                "--".to_string(),
                "example.com".to_string(),
            ],
        };
        let resolved = resolve_terminal_launch(&TerminalRuntimeConfig::default(), Some(&launch))
            .expect("typed launch");
        assert_eq!(resolved.program, "ssh");
        assert_eq!(
            resolved.args,
            vec![
                "-i".to_string(),
                "/tmp/key; touch /tmp/should-not-exist".to_string(),
                "--".to_string(),
                "example.com".to_string(),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_startup_commands_still_use_the_unix_shell() {
        let launch = TerminalLaunch::ShellCommand("printf existing-behavior".to_string());
        let resolved = resolve_terminal_launch(&TerminalRuntimeConfig::default(), Some(&launch))
            .expect("shell command launch");
        assert_eq!(resolved.program, "/bin/sh");
        assert_eq!(
            resolved.args,
            vec!["-c".to_string(), "printf existing-behavior".to_string()]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shell_setting_selects_powershell() {
        let launch = default_shell_launch(&TerminalRuntimeConfig {
            windows_shell: WindowsShell::PowerShell,
            ..TerminalRuntimeConfig::default()
        });

        assert_eq!(launch.program, "powershell.exe");
        assert_eq!(launch.args, vec!["-NoLogo".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shell_setting_selects_powershell_core() {
        let launch = default_shell_launch(&TerminalRuntimeConfig {
            windows_shell: WindowsShell::PowerShellCore,
            ..TerminalRuntimeConfig::default()
        });

        assert_eq!(launch.program, "pwsh.exe");
        assert_eq!(launch.args, vec!["-NoLogo".to_string()]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn public_resolver_preserves_every_windows_shell_launch() {
        let cases = [
            (WindowsShell::Cmd, windows_cmd_path(), Vec::new()),
            (
                WindowsShell::PowerShell,
                "powershell.exe".to_string(),
                vec!["-NoLogo".to_string()],
            ),
            (
                WindowsShell::PowerShellCore,
                "pwsh.exe".to_string(),
                vec!["-NoLogo".to_string()],
            ),
            (
                WindowsShell::GitBash,
                windows_git_bash_path(),
                vec!["--login".to_string(), "-i".to_string()],
            ),
        ];

        for (windows_shell, program, args) in cases {
            let resolved = resolve_terminal_launch(
                &TerminalRuntimeConfig {
                    windows_shell,
                    ..TerminalRuntimeConfig::default()
                },
                None,
            )
            .expect("Windows shell launch");
            assert_eq!(resolved.program, program);
            assert_eq!(resolved.args, args);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn public_resolver_preserves_every_windows_startup_command_launch() {
        let command = "echo startup";
        let cases = [
            (
                WindowsShell::Cmd,
                windows_cmd_path(),
                vec!["/C".to_string(), command.to_string()],
            ),
            (
                WindowsShell::PowerShell,
                "powershell.exe".to_string(),
                vec![
                    "-NoLogo".to_string(),
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            ),
            (
                WindowsShell::PowerShellCore,
                "pwsh.exe".to_string(),
                vec![
                    "-NoLogo".to_string(),
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-Command".to_string(),
                    command.to_string(),
                ],
            ),
            (
                WindowsShell::GitBash,
                windows_git_bash_path(),
                vec!["-lc".to_string(), command.to_string()],
            ),
        ];
        let launch = TerminalLaunch::ShellCommand(command.to_string());

        for (windows_shell, program, args) in cases {
            let resolved = resolve_terminal_launch(
                &TerminalRuntimeConfig {
                    windows_shell,
                    ..TerminalRuntimeConfig::default()
                },
                Some(&launch),
            )
            .expect("Windows startup command launch");
            assert_eq!(resolved.program, program);
            assert_eq!(resolved.args, args);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn public_resolver_preserves_custom_windows_shell_startup_behavior() {
        let runtime_config = TerminalRuntimeConfig {
            shell: Some(r"C:\Tools\custom-shell.exe".to_string()),
            windows_shell: WindowsShell::PowerShellCore,
            ..TerminalRuntimeConfig::default()
        };

        let interactive =
            resolve_terminal_launch(&runtime_config, None).expect("custom Windows shell launch");
        assert_eq!(interactive.program, r"C:\Tools\custom-shell.exe");
        assert!(interactive.args.is_empty());

        let startup = TerminalLaunch::ShellCommand("echo startup".to_string());
        let command = resolve_terminal_launch(&runtime_config, Some(&startup))
            .expect("custom Windows startup command launch");
        assert_eq!(command.program, "cmd.exe");
        assert_eq!(command.args, ["/C", "echo startup"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_startup_commands_use_selected_shell() {
        let launch = super::windows_startup_command_shell(WindowsShell::GitBash, "echo hi");

        assert_eq!(launch.args, vec!["-lc".to_string(), "echo hi".to_string()]);
    }

    #[test]
    fn core_cursor_advance_matches_for_ascii_and_starship_glyph() {
        let ascii = cursor_after_bytes(b"> ");
        let starship = cursor_after_bytes("❯ ".as_bytes());
        assert_eq!(ascii, starship);
    }

    #[test]
    fn core_cursor_advance_ignores_ansi_sequences_for_ascii_and_starship_glyph() {
        let ascii = cursor_after_bytes(b"\x1b[1;32m>\x1b[0m ");
        let starship = cursor_after_bytes("\x1b[1;32m❯\x1b[0m ".as_bytes());
        assert_eq!(ascii, starship);
    }

    #[test]
    fn core_cursor_advance_matches_after_osc_title_with_bel_terminator() {
        let ascii = cursor_after_bytes(b"\x1b]2;termy:tab:prompt:/tmp\x07> ");
        let starship = cursor_after_bytes("\x1b]2;termy:tab:prompt:/tmp\x07❯ ".as_bytes());
        assert_eq!(ascii, starship);
    }

    #[test]
    fn core_cursor_advance_matches_after_osc_title_with_st_terminator() {
        let ascii = cursor_after_bytes(b"\x1b]2;termy:tab:prompt:/tmp\x1b\\> ");
        let starship = cursor_after_bytes("\x1b]2;termy:tab:prompt:/tmp\x1b\\❯ ".as_bytes());
        assert_eq!(ascii, starship);
    }

    #[test]
    fn cursor_state_hides_and_restores_with_terminal_visibility_sequences() {
        let hidden = cursor_state_after_bytes(b"prompt\x1b[?25l", TerminalRuntimeConfig::default());
        assert_eq!(hidden, None);

        let restored = cursor_state_after_bytes(
            b"prompt\x1b[?25l\x1b[?25h",
            TerminalRuntimeConfig::default(),
        );
        assert_eq!(
            restored,
            Some(TerminalCursorState {
                col: 6,
                row: 0,
                style: TerminalCursorStyle::Block,
            })
        );
    }

    #[test]
    fn cursor_position_remains_available_when_terminal_hides_cursor() {
        assert_eq!(cursor_position_after_bytes(b"prompt\x1b[?25l"), (6, 0));
    }

    #[test]
    fn cursor_state_maps_terminal_requested_shapes_to_supported_renderer_styles() {
        let block = cursor_state_after_bytes(
            b"\x1b[2 q",
            TerminalRuntimeConfig {
                default_cursor_style: TerminalCursorStyle::Line,
                ..TerminalRuntimeConfig::default()
            },
        );
        assert_eq!(
            block,
            Some(TerminalCursorState {
                col: 0,
                row: 0,
                style: TerminalCursorStyle::Block,
            })
        );

        let underline = cursor_state_after_bytes(b"\x1b[4 q", TerminalRuntimeConfig::default());
        assert_eq!(
            underline,
            Some(TerminalCursorState {
                col: 0,
                row: 0,
                style: TerminalCursorStyle::Line,
            })
        );

        let beam = cursor_state_after_bytes(b"\x1b[6 q", TerminalRuntimeConfig::default());
        assert_eq!(
            beam,
            Some(TerminalCursorState {
                col: 0,
                row: 0,
                style: TerminalCursorStyle::Line,
            })
        );
    }

    #[test]
    fn applying_runtime_options_preserves_default_cursor_style_when_scrollback_changes() {
        let size = test_terminal_size();
        let initial = TerminalRuntimeConfig {
            scrollback_history: 256,
            default_cursor_style: TerminalCursorStyle::Line,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(size, Some(&initial));

        let updated = TerminalRuntimeConfig {
            scrollback_history: 8,
            ..initial
        };
        terminal.set_term_options(updated.term_options());
        let output = (0..80)
            .map(|index| format!("line-{index}\r\n"))
            .collect::<String>();
        terminal.feed_output(output.as_bytes());

        assert_eq!(terminal.scroll_state().1, 8);
        assert_eq!(
            terminal.cursor_state().map(|cursor| cursor.style),
            Some(TerminalCursorStyle::Line)
        );
    }

    #[test]
    fn shrinking_scrollback_trims_history_and_keeps_terminal_usable() {
        let size = test_terminal_size();
        let initial = TerminalRuntimeConfig {
            scrollback_history: 256,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(size, Some(&initial));

        let output = (0..300)
            .map(|index| format!("line-{index}\r\n"))
            .collect::<String>();
        terminal.feed_output(output.as_bytes());
        assert_eq!(terminal.scroll_state().1, 256);

        // Shrink (the inactive-tab path), which must also trim the raw buffer.
        let inactive = TerminalRuntimeConfig {
            scrollback_history: 16,
            ..initial.clone()
        };
        terminal.set_term_options(inactive.term_options());
        assert_eq!(terminal.scroll_state().1, 16);

        // Grow back (tab reactivated) and keep scrolling: storage must regrow.
        terminal.set_term_options(initial.term_options());
        terminal.feed_output(output.as_bytes());
        assert_eq!(terminal.scroll_state().1, 256);
    }

    #[test]
    fn applying_runtime_options_preserves_scrollback_when_cursor_style_changes() {
        let size = test_terminal_size();
        let initial = TerminalRuntimeConfig {
            scrollback_history: 8,
            ..TerminalRuntimeConfig::default()
        };
        let terminal = Terminal::new_display(size, Some(&initial));

        let updated = TerminalRuntimeConfig {
            default_cursor_style: TerminalCursorStyle::Line,
            ..initial
        };
        terminal.set_term_options(updated.term_options());
        let output = (0..80)
            .map(|index| format!("line-{index}\r\n"))
            .collect::<String>();
        terminal.feed_output(output.as_bytes());

        assert_eq!(terminal.scroll_state().1, 8);
        assert_eq!(
            terminal.cursor_state().map(|cursor| cursor.style),
            Some(TerminalCursorStyle::Line)
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_forces_lc_ctype_when_no_utf8_and_no_lc_all() {
        assert_eq!(
            super::utf8_locale_override_plan(None, Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::LcCtypeOnly
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_forces_lc_all_when_lc_all_is_non_utf8() {
        assert_eq!(
            super::utf8_locale_override_plan(Some("C"), Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::LcAllAndLcCtype
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_skips_when_utf8_present() {
        assert_eq!(
            super::utf8_locale_override_plan(Some("en_US.UTF-8"), Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::None
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_prefers_lc_all_over_lang() {
        assert_eq!(
            super::utf8_locale_override_plan(
                Some("fr_FR.ISO8859-1"),
                Some("C"),
                Some("en_US.UTF-8")
            ),
            super::Utf8LocaleOverridePlan::LcAllAndLcCtype
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_does_not_skip_for_utf8_substring_false_positive() {
        assert_eq!(
            super::utf8_locale_override_plan(Some("en_US.fakeutf8"), Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::LcAllAndLcCtype
        );
    }

    #[cfg(unix)]
    #[test]
    fn locale_override_plan_skips_for_utf8_with_modifier() {
        assert_eq!(
            super::utf8_locale_override_plan(Some("en_US.UTF-8@variant"), Some("C"), Some("")),
            super::Utf8LocaleOverridePlan::None
        );
    }

    #[cfg(unix)]
    #[test]
    fn preferred_utf8_locale_preserves_lang_region_from_lc_all() {
        assert_eq!(
            super::preferred_utf8_locale(
                Some("fr_FR.ISO8859-1"),
                Some("C"),
                Some("en_US.ISO8859-1")
            ),
            "fr_FR.UTF-8"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preferred_utf8_locale_preserves_locale_modifier() {
        assert_eq!(
            super::preferred_utf8_locale(None, Some("sr_RS@latin"), Some("")),
            "sr_RS.UTF-8@latin"
        );
    }

    #[cfg(unix)]
    #[test]
    fn preferred_utf8_locale_falls_back_for_c_or_posix() {
        assert_eq!(
            super::preferred_utf8_locale(Some("C"), Some("POSIX"), Some("")),
            crate::locale::DEFAULT_UTF8_LOCALE
        );
    }
}
