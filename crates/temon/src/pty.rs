use std::{
    collections::BTreeMap,
    ffi::{CString, c_char, c_int, c_ulong, c_void},
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, ErrorKind, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use crate::Size;

const F_SETFD: c_int = 2;
const FD_CLOEXEC: c_int = 1;
const SIGHUP: c_int = 1;
const SIGKILL: c_int = 9;

#[cfg(any(target_os = "linux", target_os = "android"))]
const TIOCSWINSZ: c_ulong = 0x5414;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const TIOCSWINSZ: c_ulong = 0x8008_7467;

#[repr(C)]
#[derive(Clone, Copy)]
struct WinSize {
    rows: u16,
    cols: u16,
    pixel_width: u16,
    pixel_height: u16,
}

impl From<Size> for WinSize {
    fn from(size: Size) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size
                .cell_width
                .mul_add(f32::from(size.cols), 0.0)
                .round()
                .clamp(1.0, f32::from(u16::MAX)) as u16,
            pixel_height: size
                .cell_height
                .mul_add(f32::from(size.rows), 0.0)
                .round()
                .clamp(1.0, f32::from(u16::MAX)) as u16,
        }
    }
}

#[cfg_attr(not(target_os = "android"), link(name = "util"))]
unsafe extern "C" {
    fn forkpty(
        master: *mut c_int,
        name: *mut c_char,
        termios: *const c_void,
        size: *const WinSize,
    ) -> c_int;
}

unsafe extern "C" {
    fn chdir(path: *const c_char) -> c_int;
    fn execve(
        file: *const c_char,
        argv: *const *const c_char,
        environment: *const *const c_char,
    ) -> c_int;
    fn fcntl(fd: c_int, command: c_int, value: c_int) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn _exit(status: c_int) -> !;
}

#[derive(Clone, Debug)]
pub(crate) struct SpawnConfig {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) environment: Vec<(String, String)>,
}

pub(crate) struct Pty {
    writer: mpsc::Sender<PtyCommand>,
    child_pid: c_int,
    alive: Arc<AtomicBool>,
}

enum PtyCommand {
    Write(Vec<u8>),
    Resize(WinSize),
    Close,
}

impl Pty {
    pub(crate) fn spawn(
        config: SpawnConfig,
        size: Size,
        mut on_output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        let program_path = resolve_program(&config)?;
        let program = c_string_os(program_path.as_os_str(), "terminal program")?;
        let mut arguments = Vec::with_capacity(config.args.len() + 1);
        arguments.push(c_string(&config.program, "terminal program")?);
        for argument in &config.args {
            arguments.push(c_string(argument, "terminal argument")?);
        }
        let mut argv = arguments
            .iter()
            .map(|argument| argument.as_ptr())
            .collect::<Vec<_>>();
        argv.push(ptr::null());

        let working_directory = config
            .working_directory
            .as_ref()
            .map(|path| c_string_os(path.as_os_str(), "working directory"))
            .transpose()?;
        let environment =
            child_environment(&config.environment, config.working_directory.as_deref())?;
        let mut environment_pointers = environment
            .iter()
            .map(|entry| entry.as_ptr())
            .collect::<Vec<_>>();
        environment_pointers.push(ptr::null());

        let mut master = -1;
        let window_size = WinSize::from(size);
        // SAFETY: all pointers refer to parent-owned allocations that remain valid across
        // fork. The child only invokes libc functions before replacing its image.
        let pid = unsafe { forkpty(&mut master, ptr::null_mut(), ptr::null(), &window_size) };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }

        if pid == 0 {
            if let Some(directory) = &working_directory {
                // SAFETY: `directory` is a valid, NUL-terminated C string.
                unsafe {
                    if chdir(directory.as_ptr()) < 0 {
                        _exit(126);
                    }
                }
            }
            // SAFETY: `argv` and `environment_pointers` are NUL-terminated and all
            // pointed-to strings remain alive. execve is async-signal-safe after fork.
            unsafe {
                let _ = execve(
                    program.as_ptr(),
                    argv.as_ptr(),
                    environment_pointers.as_ptr(),
                );
                _exit(127);
            }
        }

        // SAFETY: `master` is a new, owned file descriptor in the parent branch.
        let master_file = unsafe { File::from_raw_fd(master) };
        // SAFETY: fcntl is called with an owned, valid descriptor and an integer flag.
        if unsafe { fcntl(master, F_SETFD, FD_CLOEXEC) } < 0 {
            let error = io::Error::last_os_error();
            drop(master_file);
            terminate_and_reap(pid);
            return Err(error);
        }
        let mut reader = match master_file.try_clone() {
            Ok(reader) => reader,
            Err(error) => {
                drop(master_file);
                terminate_and_reap(pid);
                return Err(error);
            }
        };
        let (writer, writer_rx) = mpsc::channel();
        let writer_thread = thread::Builder::new()
            .name("tmon-pty-writer".to_string())
            .spawn(move || {
                let mut master_file = master_file;
                while let Ok(command) = writer_rx.recv() {
                    match command {
                        PtyCommand::Write(bytes) => {
                            if master_file.write_all(&bytes).is_err() {
                                break;
                            }
                        }
                        PtyCommand::Resize(window_size) => {
                            // SAFETY: this thread owns the PTY master descriptor and
                            // `window_size` has the platform winsize layout.
                            if unsafe { ioctl(master_file.as_raw_fd(), TIOCSWINSZ, &window_size) }
                                < 0
                            {
                                break;
                            }
                        }
                        PtyCommand::Close => break,
                    }
                }
            });
        if let Err(error) = writer_thread {
            drop(reader);
            terminate_and_reap(pid);
            return Err(error);
        }

        let reader_writer = writer.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();

        let reader_thread = thread::Builder::new()
            .name("tmon-pty".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 32 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            let reply = on_output(&buffer[..read]);
                            if !reply.is_empty() {
                                let _ = reader_writer.send(PtyCommand::Write(reply));
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::Interrupted => {}
                        // Linux PTYs commonly report EIO when the slave closes.
                        Err(error) if error.raw_os_error() == Some(5) => break,
                        Err(_) => break,
                    }
                }
                drop(reader);
                let _ = reader_writer.send(PtyCommand::Close);
                // SAFETY: this thread is the sole waiter for the child PID.
                unsafe {
                    let mut status = 0;
                    while waitpid(pid, &mut status, 0) < 0
                        && io::Error::last_os_error().kind() == ErrorKind::Interrupted
                    {
                    }
                }
                reader_alive.store(false, Ordering::Release);
                on_exit();
            });
        if let Err(error) = reader_thread {
            let _ = writer.send(PtyCommand::Close);
            terminate_and_reap(pid);
            return Err(error);
        }

        Ok(Self {
            writer,
            child_pid: pid,
            alive,
        })
    }

    pub(crate) fn write(&self, input: &[u8]) -> io::Result<()> {
        self.write_owned(input.to_vec())
    }

    pub(crate) fn write_owned(&self, input: Vec<u8>) -> io::Result<()> {
        self.writer
            .send(PtyCommand::Write(input))
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "tmon PTY is closed"))
    }

    pub(crate) fn resize(&self, size: Size) -> io::Result<()> {
        self.writer
            .send(PtyCommand::Resize(WinSize::from(size)))
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "tmon PTY is closed"))
    }

    pub(crate) fn child_pid(&self) -> u32 {
        self.child_pid as u32
    }
}

fn terminate_and_reap(pid: c_int) {
    // SAFETY: `pid` is the exact process and process group created by forkpty.
    unsafe {
        let _ = kill(-pid, SIGKILL);
        let _ = kill(pid, SIGKILL);
        let mut status = 0;
        while waitpid(pid, &mut status, 0) < 0
            && io::Error::last_os_error().kind() == ErrorKind::Interrupted
        {}
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if self.alive.load(Ordering::Acquire) {
            // SAFETY: signaling the exact child process created by this PTY is the intended
            // shutdown path. SIGHUP mirrors closing a conventional terminal session.
            unsafe {
                if kill(-self.child_pid, SIGHUP) < 0 {
                    let _ = kill(self.child_pid, SIGHUP);
                }
            }
        }
        let _ = self.writer.send(PtyCommand::Close);
    }
}

fn c_string(value: &str, label: &str) -> io::Result<CString> {
    CString::new(value).map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("{label} cannot contain NUL bytes"),
        )
    })
}

fn c_string_os(value: &OsStr, label: &str) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("{label} cannot contain NUL bytes"),
        )
    })
}

fn child_environment(
    overrides: &[(String, String)],
    working_directory: Option<&Path>,
) -> io::Result<Vec<CString>> {
    let mut environment = std::env::vars_os().collect::<BTreeMap<OsString, OsString>>();
    for (name, value) in overrides {
        if name.is_empty() || name.as_bytes().contains(&b'=') {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "terminal environment names cannot be empty or contain '='",
            ));
        }
        environment.insert(OsString::from(name), OsString::from(value));
    }
    if let Some(working_directory) = working_directory {
        environment.insert(
            OsString::from("PWD"),
            working_directory.as_os_str().to_os_string(),
        );
    }

    environment
        .into_iter()
        .map(|(name, value)| {
            let mut entry = Vec::with_capacity(name.as_bytes().len() + value.as_bytes().len() + 1);
            entry.extend_from_slice(name.as_bytes());
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry).map_err(|_| {
                io::Error::new(
                    ErrorKind::InvalidInput,
                    "terminal environment cannot contain NUL bytes",
                )
            })
        })
        .collect()
}

fn resolve_program(config: &SpawnConfig) -> io::Result<PathBuf> {
    let program = PathBuf::from(&config.program);
    if config.program.as_bytes().contains(&b'/') {
        return Ok(program);
    }

    let path = config
        .environment
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| OsString::from(value))
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(&program);
        if candidate
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        ErrorKind::NotFound,
        format!(
            "terminal program '{}' was not found in PATH",
            config.program
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winsize_uses_cell_metrics() {
        let size = WinSize::from(Size {
            cols: 80,
            rows: 24,
            cell_width: 9.0,
            cell_height: 18.0,
        });
        assert_eq!(size.cols, 80);
        assert_eq!(size.rows, 24);
        assert_eq!(size.pixel_width, 720);
        assert_eq!(size.pixel_height, 432);
    }

    #[test]
    fn child_environment_rejects_invalid_override_names() {
        for name in ["", "BAD=NAME"] {
            let error = child_environment(&[(name.to_string(), "value".to_string())], None)
                .expect_err("invalid environment name should fail");
            assert_eq!(error.kind(), ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn child_environment_reports_the_actual_working_directory() {
        let directory = Path::new("/tmp/tmon working directory");
        let environment = child_environment(
            &[("PWD".to_string(), "/stale/value".to_string())],
            Some(directory),
        )
        .expect("working directory should be representable in the child environment");
        assert!(
            environment
                .iter()
                .any(|entry| { entry.as_bytes() == b"PWD=/tmp/tmon working directory" })
        );
    }
}
