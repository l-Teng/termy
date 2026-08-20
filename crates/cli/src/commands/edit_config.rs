use std::{ffi::OsString, path::Path, process::Command};
use termy_config_core::config_path;

struct EditorLauncher {
    program: OsString,
    args: Vec<OsString>,
}

impl EditorLauncher {
    fn new(program: impl Into<OsString>, args: Vec<OsString>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }
}

pub fn run() -> Result<(), String> {
    let Some(path) = config_path() else {
        return Err("Could not determine config directory".to_string());
    };

    if !path.exists() {
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Err(format!("Failed to create config directory: {e}"));
        }
        if let Err(e) = std::fs::write(&path, "") {
            return Err(format!("Failed to create config file: {e}"));
        }
    }

    println!("Opening {}", path.display());
    launch_editor(&path, std::env::var_os("EDITOR"))
}

fn launch_editor(path: &Path, editor: Option<OsString>) -> Result<(), String> {
    let launchers = editor_launchers(path, editor);
    try_launchers(&launchers, |launcher| {
        Command::new(&launcher.program)
            .args(&launcher.args)
            .status()
            .map(|status| status.success())
    })
}

fn editor_launchers(path: &Path, editor: Option<OsString>) -> Vec<EditorLauncher> {
    let path = path.as_os_str().to_os_string();
    let mut launchers = Vec::new();

    // Try $EDITOR first, then platform-specific fallbacks
    if let Some(editor) = editor {
        launchers.push(EditorLauncher::new(editor, vec![path.clone()]));
    }

    #[cfg(target_os = "macos")]
    launchers.push(EditorLauncher::new(
        "open",
        vec![OsString::from("-t"), path.clone()],
    ));

    #[cfg(target_os = "linux")]
    {
        launchers.push(EditorLauncher::new("xdg-open", vec![path.clone()]));
        for editor in ["nano", "vim", "vi"] {
            launchers.push(EditorLauncher::new(editor, vec![path.clone()]));
        }
    }

    #[cfg(target_os = "windows")]
    launchers.push(EditorLauncher::new("notepad", vec![path]));

    launchers
}

fn try_launchers<F>(launchers: &[EditorLauncher], mut launch: F) -> Result<(), String>
where
    F: FnMut(&EditorLauncher) -> std::io::Result<bool>,
{
    let mut failures = Vec::with_capacity(launchers.len());
    for launcher in launchers {
        let name = launcher.program.to_string_lossy();
        match launch(launcher) {
            Ok(true) => return Ok(()),
            Ok(false) => failures.push(format!("{name} exited with an error")),
            Err(error) => failures.push(format!("failed to run {name}: {error}")),
        }
    }

    Err(format!(
        "Could not open the config file: {}",
        failures.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, io};

    #[test]
    fn launcher_failures_fall_through_until_one_succeeds() {
        let launchers = vec![
            EditorLauncher::new("preferred", Vec::new()),
            EditorLauncher::new("fallback", Vec::new()),
        ];
        let mut outcomes = VecDeque::from([
            Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            Ok(true),
        ]);
        let mut attempted = Vec::new();

        let result = try_launchers(&launchers, |launcher| {
            attempted.push(launcher.program.clone());
            outcomes.pop_front().expect("launcher outcome")
        });

        assert_eq!(result, Ok(()));
        assert_eq!(
            attempted,
            [OsString::from("preferred"), OsString::from("fallback")]
        );
    }

    #[test]
    fn nonzero_exit_is_reported_when_no_launcher_succeeds() {
        let launchers = vec![
            EditorLauncher::new("preferred", Vec::new()),
            EditorLauncher::new("fallback", Vec::new()),
        ];

        let error = try_launchers(&launchers, |_| Ok(false))
            .expect_err("all unsuccessful launchers should fail");

        assert_eq!(
            error,
            "Could not open the config file: preferred exited with an error; fallback exited with an error"
        );
    }
}
