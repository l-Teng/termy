use std::{
    collections::BTreeMap,
    ffi::{CString, c_char, c_int, c_short, c_ulong, c_void},
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, ErrorKind, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::net::UnixStream,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use crate::Size;

const F_GETFL: c_int = 3;
const F_SETFD: c_int = 2;
const F_SETFL: c_int = 4;
const FD_CLOEXEC: c_int = 1;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: c_int = 0x0800;
#[cfg(target_os = "macos")]
const O_NONBLOCK: c_int = 0x0004;
const EINTR: c_int = 4;
const SIGHUP: c_int = 1;
const SIGKILL: c_int = 9;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGINT: c_int = 2;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGQUIT: c_int = 3;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGALRM: c_int = 14;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGTERM: c_int = 15;
#[cfg(any(target_os = "linux", target_os = "android"))]
const SIGCHLD: c_int = 17;
#[cfg(target_os = "macos")]
const SIGCHLD: c_int = 20;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const SIGNAL_DEFAULT: usize = 0;
const POLLIN: c_short = 0x0001;
const POLLOUT: c_short = 0x0004;
const POLLERR: c_short = 0x0008;
const POLLHUP: c_short = 0x0010;
const POLLNVAL: c_short = 0x0020;
const CHILD_SPAWN_ERROR_LENGTH: usize = 5;
const MAX_EXIT_DRAIN_READS: usize = 256;
const WRITER_POLL_TIMEOUT_MS: c_int = 10;

#[cfg(any(target_os = "linux", target_os = "android"))]
type PollCount = c_ulong;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
type PollCount = u32;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
type TerminalInputFlags = u32;
#[cfg(target_os = "macos")]
type TerminalInputFlags = c_ulong;

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const IUTF8: TerminalInputFlags = 0x0000_4000;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const TCSANOW: c_int = 0;
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
const STDIN_FILENO: c_int = 0;

/// Opaque, over-allocated storage for the platform's `struct termios`.
///
/// POSIX places `c_iflag` first, but Darwin and Linux use different flag widths
/// and full structure layouts. Keeping the remainder opaque avoids encoding
/// libc-specific padding while preserving enough size and alignment for both.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
#[repr(C)]
struct TermiosStorage {
    input_flags: TerminalInputFlags,
    opaque: [u8; 256],
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const TIOCSWINSZ: c_ulong = 0x5414;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const TIOCSWINSZ: c_ulong = 0x8008_7467;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    fn close(fd: c_int) -> c_int;
    fn execve(
        file: *const c_char,
        argv: *const *const c_char,
        environment: *const *const c_char,
    ) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn poll(descriptors: *mut PollFd, count: PollCount, timeout: c_int) -> c_int;
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    fn signal(signal: c_int, handler: usize) -> usize;
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    fn tcgetattr(fd: c_int, termios: *mut TermiosStorage) -> c_int;
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    fn tcsetattr(fd: c_int, actions: c_int, termios: *const TermiosStorage) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    #[link_name = "write"]
    fn write_fd(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    fn _exit(status: c_int) -> !;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn __errno_location() -> *mut c_int;
    #[cfg(target_os = "macos")]
    fn __error() -> *mut c_int;
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum ChildSpawnStage {
    ChangeDirectory = 1,
    Execute = 2,
}

impl ChildSpawnStage {
    fn from_byte(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::ChangeDirectory),
            2 => Some(Self::Execute),
            _ => None,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::ChangeDirectory => "change terminal working directory",
            Self::Execute => "execute terminal program",
        }
    }
}

struct ChildSpawnFailure {
    stage: ChildSpawnStage,
    errno: c_int,
}

#[derive(Clone, Debug)]
pub(crate) struct SpawnConfig {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) environment: Vec<(String, String)>,
}

pub(crate) struct Pty {
    writer: WriterHandle,
    child_pid: c_int,
    alive: Arc<AtomicBool>,
}

#[derive(Clone)]
struct WriterHandle {
    sender: mpsc::Sender<WriterCommand>,
    control: Arc<WriterControl>,
}

#[derive(Default)]
struct WriterControl {
    close_requested: AtomicBool,
    pending_resize: Mutex<Option<WinSize>>,
}

enum WriterCommand {
    Write(Vec<u8>),
    Wake,
}

impl WriterHandle {
    fn channel() -> (Self, mpsc::Receiver<WriterCommand>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                sender,
                control: Arc::new(WriterControl::default()),
            },
            receiver,
        )
    }

    fn write_owned(&self, bytes: Vec<u8>) -> io::Result<()> {
        if self.control.is_closed() {
            return Err(writer_closed_error());
        }
        self.sender
            .send(WriterCommand::Write(bytes))
            .map_err(|_| writer_closed_error())
    }

    fn resize(&self, window_size: WinSize) -> io::Result<()> {
        if !self.control.request_resize(window_size) {
            return Err(writer_closed_error());
        }
        self.sender
            .send(WriterCommand::Wake)
            .map_err(|_| writer_closed_error())
    }

    fn close(&self) {
        self.control.request_close();
        // The receiver may be sleeping with no writes queued. A wake command
        // makes the sticky close visible without waiting for channel teardown.
        let _ = self.sender.send(WriterCommand::Wake);
    }
}

impl WriterControl {
    fn is_closed(&self) -> bool {
        self.close_requested.load(Ordering::Acquire)
    }

    fn request_resize(&self, window_size: WinSize) -> bool {
        if self.is_closed() {
            return false;
        }

        let mut pending_resize = self
            .pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.is_closed() {
            return false;
        }
        *pending_resize = Some(window_size);
        true
    }

    fn take_resize(&self) -> Option<WinSize> {
        self.pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn request_close(&self) {
        self.close_requested.store(true, Ordering::Release);
        // Do not leave stale resize state behind after cancellation. The lock
        // is never held while the writer performs ioctl, write, or poll.
        self.pending_resize
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

fn writer_closed_error() -> io::Error {
    io::Error::new(ErrorKind::BrokenPipe, "tmon PTY is closed")
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

        let (mut child_status_reader, child_status_writer) = UnixStream::pair()?;
        // A successful exec closes the child endpoint and reports EOF to the
        // parent. Failures write a small stage + errno record before _exit.
        if unsafe { fcntl(child_status_writer.as_raw_fd(), F_SETFD, FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut master = -1;
        let window_size = WinSize::from(size);
        // SAFETY: all pointers refer to parent-owned allocations that remain valid across
        // fork. The child only invokes libc functions before replacing its image.
        let pid = unsafe { forkpty(&mut master, ptr::null_mut(), ptr::null(), &window_size) };
        if pid < 0 {
            return Err(io::Error::last_os_error());
        }

        if pid == 0 {
            // SAFETY: the child does not use the parent's status endpoint.
            unsafe {
                let _ = close(child_status_reader.as_raw_fd());
            }
            reset_child_process_state();
            if let Some(directory) = &working_directory {
                // SAFETY: `directory` is a valid, NUL-terminated C string.
                unsafe {
                    if chdir(directory.as_ptr()) < 0 {
                        report_child_spawn_failure(
                            child_status_writer.as_raw_fd(),
                            ChildSpawnStage::ChangeDirectory,
                        );
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
                report_child_spawn_failure(
                    child_status_writer.as_raw_fd(),
                    ChildSpawnStage::Execute,
                );
                _exit(127);
            }
        }

        drop(child_status_writer);
        // SAFETY: `master` is a new, owned file descriptor in the parent branch.
        let master_file = unsafe { File::from_raw_fd(master) };
        match read_child_spawn_status(&mut child_status_reader) {
            Ok(None) => {}
            Ok(Some(failure)) => {
                drop(master_file);
                wait_for_child(pid);
                return Err(failure.into_io_error());
            }
            Err(error) => {
                drop(master_file);
                terminate_and_reap(pid);
                return Err(error);
            }
        }
        drop(child_status_reader);
        // SAFETY: fcntl is called with an owned, valid descriptor and an integer flag.
        if unsafe { fcntl(master, F_SETFD, FD_CLOEXEC) } < 0 {
            let error = io::Error::last_os_error();
            drop(master_file);
            terminate_and_reap(pid);
            return Err(error);
        }
        if let Err(error) = set_nonblocking(&master_file) {
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
        let (writer, writer_rx) = WriterHandle::channel();
        let writer_control = writer.control.clone();
        let writer_thread = thread::Builder::new()
            .name("tmon-pty-writer".to_string())
            .spawn(move || run_pty_writer(master_file, writer_rx, writer_control));
        if let Err(error) = writer_thread {
            drop(reader);
            terminate_and_reap(pid);
            return Err(error);
        }

        let (mut shutdown_reader, mut shutdown_writer) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(error) => {
                writer.close();
                terminate_and_reap(pid);
                return Err(error);
            }
        };
        let reader_writer = writer.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();

        let reader_thread = thread::Builder::new()
            .name("tmon-pty".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 32 * 1024];
                let mut child_wakeup = false;
                let mut descriptors = [
                    PollFd {
                        fd: reader.as_raw_fd(),
                        events: POLLIN,
                        revents: 0,
                    },
                    PollFd {
                        fd: shutdown_reader.as_raw_fd(),
                        events: POLLIN,
                        revents: 0,
                    },
                ];
                loop {
                    descriptors[0].revents = 0;
                    descriptors[1].revents = 0;
                    // SAFETY: both descriptors remain owned by this thread for the
                    // duration of the call, and the array has exactly two entries.
                    let ready = unsafe {
                        poll(descriptors.as_mut_ptr(), descriptors.len() as PollCount, -1)
                    };
                    if ready < 0 {
                        if io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                            continue;
                        }
                        break;
                    }

                    let shutdown_events = descriptors[1].revents;
                    if shutdown_events & (POLLIN | POLLERR | POLLHUP | POLLNVAL) != 0 {
                        child_wakeup = true;
                        drain_ready_pty_output(
                            &mut reader,
                            &mut buffer,
                            &mut on_output,
                            &reader_writer,
                        );
                        break;
                    }

                    let reader_events = descriptors[0].revents;
                    if reader_events & POLLIN == 0 {
                        if reader_events & (POLLERR | POLLHUP | POLLNVAL) != 0 {
                            drain_ready_pty_output(
                                &mut reader,
                                &mut buffer,
                                &mut on_output,
                                &reader_writer,
                            );
                            break;
                        }
                        continue;
                    }
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            let reply = on_output(&buffer[..read]);
                            if !reply.is_empty() {
                                let _ = reader_writer.write_owned(reply);
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::Interrupted => {}
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                        // Linux PTYs commonly report EIO when the slave closes.
                        Err(error) if error.raw_os_error() == Some(5) => break,
                        Err(_) => break,
                    }
                }
                drop(reader);
                reader_writer.close();
                if !child_wakeup {
                    let mut wake = [0u8; 1];
                    while shutdown_reader
                        .read(&mut wake)
                        .is_err_and(|error| error.kind() == ErrorKind::Interrupted)
                    {
                    }
                }
                if !reader_alive.load(Ordering::Acquire) {
                    on_exit();
                }
            });
        if let Err(error) = reader_thread {
            writer.close();
            terminate_and_reap(pid);
            return Err(error);
        }

        let watcher_alive = alive.clone();
        let watcher_thread = thread::Builder::new()
            .name("tmon-pty-child".to_string())
            .spawn(move || {
                wait_for_child(pid);
                watcher_alive.store(false, Ordering::Release);
                let _ = shutdown_writer.write(&[1]);
            });
        if let Err(error) = watcher_thread {
            writer.close();
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
        self.writer.write_owned(input)
    }

    pub(crate) fn resize(&self, size: Size) -> io::Result<()> {
        self.writer.resize(WinSize::from(size))
    }

    pub(crate) fn child_pid(&self) -> u32 {
        self.child_pid as u32
    }
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    // SAFETY: `file` owns a live descriptor and F_GETFL takes no variadic argument.
    let flags = unsafe { fcntl(file.as_raw_fd(), F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }

    // O_NONBLOCK is an open-file status flag, so the reader clone intentionally
    // shares it with this descriptor. Both read and write paths handle WouldBlock.
    // SAFETY: `file` owns a live descriptor and `flags | O_NONBLOCK` preserves
    // all existing open-file status flags.
    if unsafe { fcntl(file.as_raw_fd(), F_SETFL, flags | O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn run_pty_writer(
    master: File,
    receiver: mpsc::Receiver<WriterCommand>,
    control: Arc<WriterControl>,
) {
    let _ = run_pty_writer_loop(
        master,
        receiver,
        control,
        wait_for_pty_writable,
        resize_pty_master,
    );
}

fn run_pty_writer_loop<W, WaitWritable, Resize>(
    mut writer: W,
    receiver: mpsc::Receiver<WriterCommand>,
    control: Arc<WriterControl>,
    mut wait_writable: WaitWritable,
    mut resize: Resize,
) -> W
where
    W: Write,
    WaitWritable: FnMut(&W) -> io::Result<()>,
    Resize: FnMut(&W, WinSize) -> io::Result<()>,
{
    // Keep exactly one write outside the channel. All later writes remain in
    // std's unbounded FIFO, so control work never has to scan or copy backlog.
    let mut current_write: Option<(Vec<u8>, usize)> = None;

    loop {
        if !matches!(
            service_writer_control(&writer, &control, &mut resize),
            Ok(true)
        ) {
            break;
        }

        if current_write.is_none() {
            let Ok(command) = receiver.recv() else {
                break;
            };
            match command {
                WriterCommand::Write(bytes) if !bytes.is_empty() => {
                    current_write = Some((bytes, 0));
                }
                WriterCommand::Write(_) | WriterCommand::Wake => {}
            }

            // Resize and Close are published before their Wake command. Check
            // shared control state before writing even when recv returned an
            // older Write that was already ahead of that wake in the channel.
            if !matches!(
                service_writer_control(&writer, &control, &mut resize),
                Ok(true)
            ) {
                break;
            }
        }

        let Some((bytes, offset)) = current_write.as_mut() else {
            continue;
        };
        match writer.write(&bytes[*offset..]) {
            Ok(0) => break,
            Ok(written) => {
                *offset += written;
                if *offset == bytes.len() {
                    current_write = None;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if wait_writable(&writer).is_err() {
                    break;
                }
                // poll is capped at 10ms in production; service controls as
                // soon as it returns, whether for readiness or timeout.
                if !matches!(
                    service_writer_control(&writer, &control, &mut resize),
                    Ok(true)
                ) {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    writer
}

fn service_writer_control<W>(
    writer: &W,
    control: &WriterControl,
    resize: &mut impl FnMut(&W, WinSize) -> io::Result<()>,
) -> io::Result<bool> {
    if control.is_closed() {
        return Ok(false);
    }

    let pending_resize = control.take_resize();
    if control.is_closed() {
        return Ok(false);
    }
    if let Some(window_size) = pending_resize {
        resize(writer, window_size)?;
    }
    Ok(!control.is_closed())
}

fn wait_for_pty_writable(master: &File) -> io::Result<()> {
    let mut descriptor = PollFd {
        fd: master.as_raw_fd(),
        events: POLLOUT,
        revents: 0,
    };
    // The timeout bounds Resize/Close latency because std's channel and poll
    // cannot be waited on together. Readiness still wakes immediately when the
    // child drains input, avoiding a fixed delay between partial writes.
    // SAFETY: `descriptor` contains the master's live fd and exactly one entry
    // is passed to poll for the duration of the call.
    let ready = unsafe { poll(&mut descriptor, 1 as PollCount, WRITER_POLL_TIMEOUT_MS) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
    if descriptor.revents & POLLNVAL != 0 {
        return Err(io::Error::new(
            ErrorKind::BrokenPipe,
            "tmon PTY writer descriptor is invalid",
        ));
    }
    Ok(())
}

fn resize_pty_master(master: &File, window_size: WinSize) -> io::Result<()> {
    loop {
        // SAFETY: `master` owns a live PTY descriptor and `window_size` has the
        // platform winsize layout for the duration of the ioctl call.
        if unsafe { ioctl(master.as_raw_fd(), TIOCSWINSZ, &window_size) } >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

impl ChildSpawnFailure {
    fn into_io_error(self) -> io::Error {
        let source = io::Error::from_raw_os_error(self.errno);
        io::Error::new(
            source.kind(),
            format!("failed to {}: {source}", self.stage.description()),
        )
    }
}

fn read_child_spawn_status(
    status_reader: &mut UnixStream,
) -> io::Result<Option<ChildSpawnFailure>> {
    let mut record = [0u8; CHILD_SPAWN_ERROR_LENGTH];
    let mut filled = 0;
    loop {
        match status_reader.read(&mut record[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "terminal child returned an incomplete spawn status",
                ));
            }
            Ok(read) => {
                filled += read;
                if filled == record.len() {
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }

    let stage = ChildSpawnStage::from_byte(record[0]).ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidData,
            "terminal child returned an invalid spawn stage",
        )
    })?;
    let errno = c_int::from_ne_bytes([record[1], record[2], record[3], record[4]]);
    Ok(Some(ChildSpawnFailure { stage, errno }))
}

fn report_child_spawn_failure(status_fd: c_int, stage: ChildSpawnStage) {
    let errno = current_errno();
    let mut record = [0u8; CHILD_SPAWN_ERROR_LENGTH];
    record[0] = stage as u8;
    record[1..].copy_from_slice(&errno.to_ne_bytes());

    let mut written = 0;
    while written < record.len() {
        // SAFETY: `status_fd` is the child endpoint of the status socket, and
        // the remaining record bytes stay valid for the duration of the call.
        let result = unsafe {
            write_fd(
                status_fd,
                record.as_ptr().add(written).cast::<c_void>(),
                record.len() - written,
            )
        };
        if result > 0 {
            written += result as usize;
        } else if result == 0 || current_errno() != EINTR {
            break;
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn current_errno() -> c_int {
    // SAFETY: libc returns a valid pointer to this thread's errno slot.
    unsafe { *__errno_location() }
}

#[cfg(target_os = "macos")]
fn current_errno() -> c_int {
    // SAFETY: libc returns a valid pointer to this thread's errno slot.
    unsafe { *__error() }
}

fn drain_ready_pty_output(
    reader: &mut File,
    buffer: &mut [u8],
    on_output: &mut impl FnMut(&[u8]) -> Vec<u8>,
    writer: &WriterHandle,
) {
    // A descendant can keep the slave open or continue producing output. Poll
    // with a zero timeout and cap reads so direct-child exit stays prompt while
    // preserving all data that was already queued in the PTY's bounded buffer.
    for _ in 0..MAX_EXIT_DRAIN_READS {
        let mut descriptor = PollFd {
            fd: reader.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: `descriptor` contains the reader's live fd and one entry is
        // passed to poll. A zero timeout never waits for descendant activity.
        let ready = unsafe { poll(&mut descriptor, 1 as PollCount, 0) };
        if ready <= 0
            || descriptor.revents & POLLNVAL != 0
            || descriptor.revents & (POLLIN | POLLERR | POLLHUP) == 0
        {
            return;
        }

        match reader.read(buffer) {
            Ok(0) => return,
            Ok(read) => {
                let reply = on_output(&buffer[..read]);
                if !reply.is_empty() {
                    let _ = writer.write_owned(reply);
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => return,
            // Linux PTYs commonly report EIO when the slave closes.
            Err(error) if error.raw_os_error() == Some(5) => return,
            Err(_) => return,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn reset_child_process_state() {
    // Match the native Alacritty PTY path. `signal`, `tcgetattr`, and
    // `tcsetattr` are async-signal-safe, and this code performs no allocation
    // or locking between forkpty and execve.
    unsafe {
        for signal_number in [SIGCHLD, SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGALRM] {
            let _ = signal(signal_number, SIGNAL_DEFAULT);
        }

        let mut termios = TermiosStorage {
            input_flags: 0,
            opaque: [0; 256],
        };
        if tcgetattr(STDIN_FILENO, &mut termios) == 0 {
            termios.input_flags |= IUTF8;
            let _ = tcsetattr(STDIN_FILENO, TCSANOW, &termios);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn reset_child_process_state() {}

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

fn wait_for_child(pid: c_int) {
    // SAFETY: the dedicated watcher is the sole waiter for the exact child PID
    // returned by forkpty.
    unsafe {
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
        self.writer.close();
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

    let child_directory = match config.working_directory.as_deref() {
        Some(directory) if directory.is_absolute() => directory.to_path_buf(),
        Some(directory) => std::env::current_dir()?.join(directory),
        None => std::env::current_dir()?,
    };

    let path = config
        .environment
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| OsString::from(value))
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let directory = if directory.is_absolute() {
            directory
        } else {
            child_directory.join(directory)
        };
        let Ok(candidate) = directory.join(&program).canonicalize() else {
            continue;
        };
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
    use std::os::fd::IntoRawFd;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use std::process::Command;
    use std::{sync::atomic::AtomicUsize, time::Duration};

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    const SIGNAL_IGNORE: usize = 1;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    const SIGNAL_ERROR: usize = usize::MAX;

    static TEST_DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct KillProcessOnDrop(c_int);

    impl Drop for KillProcessOnDrop {
        fn drop(&mut self) {
            // SAFETY: tests construct this guard from the exact descendant PID
            // reported by the shell that they just spawned.
            unsafe {
                let _ = kill(self.0, SIGKILL);
            }
        }
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "tmon-pty-{label}-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("test directory should be created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_test_file(path: &Path, contents: &[u8], mode: u32) {
        std::fs::write(path, contents).expect("test file should be written");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("test file permissions should be set");
    }

    fn assert_spawn_error(config: SpawnConfig, expected_kind: ErrorKind) -> io::Error {
        let output_count = Arc::new(AtomicUsize::new(0));
        let callback_output_count = output_count.clone();
        let exit_count = Arc::new(AtomicUsize::new(0));
        let callback_exit_count = exit_count.clone();
        let result = Pty::spawn(
            config,
            test_size(),
            move |_| {
                callback_output_count.fetch_add(1, Ordering::AcqRel);
                Vec::new()
            },
            move || {
                callback_exit_count.fetch_add(1, Ordering::AcqRel);
            },
        );
        let error = match result {
            Ok(terminal) => {
                drop(terminal);
                panic!("invalid PTY configuration unexpectedly started a session");
            }
            Err(error) => error,
        };
        assert_eq!(error.kind(), expected_kind);
        assert_eq!(output_count.load(Ordering::Acquire), 0);
        assert_eq!(exit_count.load(Ordering::Acquire), 0);
        error
    }

    fn test_size() -> Size {
        Size {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 16.0,
        }
    }

    #[derive(Default)]
    struct PartialWriter {
        bytes: Vec<u8>,
        max_write: usize,
        write_calls: usize,
    }

    impl Write for PartialWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.write_calls += 1;
            let written = bytes.len().min(self.max_write);
            self.bytes.extend_from_slice(&bytes[..written]);
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

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
    fn writer_control_coalesces_to_the_latest_resize() {
        let (writer, receiver) = WriterHandle::channel();
        let first = WinSize::from(test_size());
        let latest = WinSize::from(Size {
            cols: 132,
            rows: 43,
            ..test_size()
        });

        writer.resize(first).expect("first resize should queue");
        writer.resize(latest).expect("latest resize should queue");
        let mut applied = Vec::new();
        assert!(
            service_writer_control(&(), &writer.control, &mut |_, window_size| {
                applied.push(window_size);
                Ok(())
            })
            .expect("resize control should apply")
        );

        assert_eq!(applied, vec![latest]);
        assert_eq!(receiver.try_iter().count(), 2, "every resize should wake");
    }

    #[test]
    fn writer_control_reapplies_a_same_size_nudge() {
        let (writer, _receiver) = WriterHandle::channel();
        let window_size = WinSize::from(test_size());
        let mut applied = Vec::new();

        for _ in 0..2 {
            writer
                .resize(window_size)
                .expect("same-size resize should queue");
            assert!(
                service_writer_control(&(), &writer.control, &mut |_, applied_size| {
                    applied.push(applied_size);
                    Ok(())
                })
                .expect("same-size resize should apply")
            );
        }

        assert_eq!(applied, vec![window_size, window_size]);
    }

    #[test]
    fn writer_control_close_is_sticky_and_cancels_pending_work() {
        let (writer, receiver) = WriterHandle::channel();
        writer
            .resize(WinSize::from(test_size()))
            .expect("resize should queue before close");
        writer.close();

        let mut resize_called = false;
        assert!(
            !service_writer_control(&(), &writer.control, &mut |_, _| {
                resize_called = true;
                Ok(())
            })
            .expect("close control should be readable")
        );
        assert!(!resize_called, "close should cancel the pending resize");
        assert!(writer.control.is_closed());
        assert_eq!(
            writer
                .write_owned(b"discarded".to_vec())
                .unwrap_err()
                .kind(),
            ErrorKind::BrokenPipe
        );
        assert_eq!(
            writer
                .resize(WinSize::from(test_size()))
                .unwrap_err()
                .kind(),
            ErrorKind::BrokenPipe
        );
        assert!(
            matches!(
                receiver.recv_timeout(Duration::from_millis(100)),
                Ok(WriterCommand::Wake)
            ),
            "resize and close should leave a wake available to an idle receiver"
        );
    }

    #[test]
    fn writer_loop_keeps_partial_writes_fifo() {
        let (writer, receiver) = WriterHandle::channel();
        let control = writer.control.clone();
        let first = b"AAAAA".to_vec();
        let second = b"BBBBB".to_vec();
        writer
            .write_owned(first.clone())
            .expect("first write should queue");
        writer
            .write_owned(second.clone())
            .expect("second write should queue");
        drop(writer);

        let sink = run_pty_writer_loop(
            PartialWriter {
                max_write: 2,
                ..PartialWriter::default()
            },
            receiver,
            control,
            |_| Ok(()),
            |_, _| Ok(()),
        );

        let mut expected = first;
        expected.extend_from_slice(&second);
        assert_eq!(sink.bytes, expected);
        assert_eq!(sink.write_calls, 6, "both writes should be partial");
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

    #[test]
    fn missing_absolute_executable_fails_synchronously() {
        let directory = TestDirectory::new("missing-executable");
        let error = assert_spawn_error(
            SpawnConfig {
                program: directory
                    .path()
                    .join("does-not-exist")
                    .to_string_lossy()
                    .into_owned(),
                args: Vec::new(),
                working_directory: None,
                environment: Vec::new(),
            },
            ErrorKind::NotFound,
        );
        assert!(error.to_string().contains("execute terminal program"));
    }

    #[test]
    fn non_executable_file_fails_synchronously() {
        let directory = TestDirectory::new("non-executable");
        let program = directory.path().join("program");
        write_test_file(&program, b"#!/bin/sh\nexit 0\n", 0o644);

        let error = assert_spawn_error(
            SpawnConfig {
                program: program.to_string_lossy().into_owned(),
                args: Vec::new(),
                working_directory: None,
                environment: Vec::new(),
            },
            ErrorKind::PermissionDenied,
        );
        assert!(error.to_string().contains("execute terminal program"));
    }

    #[test]
    fn missing_working_directory_fails_synchronously() {
        let directory = TestDirectory::new("missing-working-directory");
        let error = assert_spawn_error(
            SpawnConfig {
                program: "/bin/sh".to_string(),
                args: Vec::new(),
                working_directory: Some(directory.path().join("does-not-exist")),
                environment: Vec::new(),
            },
            ErrorKind::NotFound,
        );
        assert!(
            error
                .to_string()
                .contains("change terminal working directory")
        );
    }

    #[test]
    fn relative_path_entry_uses_child_working_directory() {
        const SENTINEL: &[u8] = b"TMON_CHILD_CWD_PATH";

        let directory = TestDirectory::new("relative-path");
        let child_directory = directory.path().join("child");
        let bin_directory = child_directory.join("bin");
        std::fs::create_dir_all(&bin_directory).expect("child bin directory should be created");
        let program = bin_directory.join("tmon-relative-path-probe");
        write_test_file(&program, b"#!/bin/sh\nprintf TMON_CHILD_CWD_PATH\n", 0o755);
        let config = SpawnConfig {
            program: "tmon-relative-path-probe".to_string(),
            args: Vec::new(),
            working_directory: Some(child_directory),
            environment: vec![("PATH".to_string(), "bin".to_string())],
        };

        let resolved = resolve_program(&config).expect("relative PATH entry should resolve");
        assert!(resolved.is_absolute());
        assert_eq!(
            resolved,
            program
                .canonicalize()
                .expect("test executable should canonicalize")
        );

        let (sentinel_tx, sentinel_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let mut output = Vec::new();
        let mut sentinel_reported = false;
        let terminal = Pty::spawn(
            config,
            test_size(),
            move |bytes| {
                output.extend_from_slice(bytes);
                if !sentinel_reported && output.windows(SENTINEL.len()).any(|part| part == SENTINEL)
                {
                    sentinel_reported = true;
                    let _ = sentinel_tx.send(());
                }
                Vec::new()
            },
            move || {
                let _ = exit_tx.send(());
            },
        )
        .expect("child-cwd relative PATH executable should start");

        sentinel_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("resolved child executable should produce its sentinel");
        exit_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("resolved child executable should exit");
        drop(terminal);
    }

    #[test]
    fn zero_timeout_exit_drain_reads_buffered_data_without_waiting_for_eof() {
        const SENTINEL: &[u8] = b"TMON_TAIL_SENTINEL";

        let (reader_stream, mut writer_stream) =
            UnixStream::pair().expect("socket pair should open");
        writer_stream
            .write_all(SENTINEL)
            .expect("sentinel should be buffered");
        // SAFETY: ownership of the socket fd is transferred into `reader`.
        let mut reader = unsafe { File::from_raw_fd(reader_stream.into_raw_fd()) };
        let (writer, _writer_rx) = WriterHandle::channel();
        let mut buffer = [0u8; 32 * 1024];
        let mut output = Vec::new();
        drain_ready_pty_output(
            &mut reader,
            &mut buffer,
            &mut |bytes| {
                output.extend_from_slice(bytes);
                Vec::new()
            },
            &writer,
        );

        assert_eq!(output, SENTINEL);
        // The peer deliberately remains open through the drain, proving the
        // zero-timeout loop did not wait for EOF or descendant-style closure.
        writer_stream
            .write_all(b"still-open")
            .expect("drain should not close the peer");
    }

    #[test]
    fn direct_child_exit_delivers_final_sentinel_before_exit_callback() {
        const SENTINEL: &[u8] = b"TMON_FINAL_SENTINEL";

        let sentinel_seen = Arc::new(AtomicBool::new(false));
        let output_sentinel_seen = sentinel_seen.clone();
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let mut output = Vec::new();
        let terminal = Pty::spawn(
            SpawnConfig {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "printf 'TMON_FINAL_SENTINEL\\n'; exit 0".to_string(),
                ],
                working_directory: None,
                environment: Vec::new(),
            },
            test_size(),
            move |bytes| {
                output.extend_from_slice(bytes);
                if output.windows(SENTINEL.len()).any(|part| part == SENTINEL) {
                    output_sentinel_seen.store(true, Ordering::Release);
                }
                Vec::new()
            },
            move || {
                let _ = exit_tx.send(sentinel_seen.load(Ordering::Acquire));
            },
        )
        .expect("PTY should start");

        assert!(
            exit_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("exit callback should run after delivering the final sentinel"),
            "exit callback ran before the final PTY sentinel was delivered"
        );
        drop(terminal);
    }

    #[test]
    fn direct_child_exit_is_reported_while_descendant_holds_slave_open() {
        const MARKER: &[u8] = b"TMON_DESCENDANT_READY:";

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let mut output = Vec::new();
        let mut acknowledged = false;
        let exit_count = Arc::new(AtomicUsize::new(0));
        let callback_count = exit_count.clone();
        let (exit_tx, exit_rx) = mpsc::channel();
        let exit_sender_guard = exit_tx.clone();

        let terminal = Pty::spawn(
            SpawnConfig {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    concat!(
                        "trap '' HUP\n",
                        "sleep 30 &\n",
                        "printf 'TMON_DESCENDANT_READY:%s\\n' \"$!\"\n",
                        "read confirmation\n",
                        "exit 0\n",
                    )
                    .to_string(),
                ],
                working_directory: None,
                environment: Vec::new(),
            },
            test_size(),
            move |bytes| {
                output.extend_from_slice(bytes);
                if acknowledged {
                    return Vec::new();
                }

                let Some(marker) = output.windows(MARKER.len()).position(|part| part == MARKER)
                else {
                    return Vec::new();
                };
                let pid_start = marker + MARKER.len();
                let Some(line_length) = output[pid_start..].iter().position(|byte| *byte == b'\n')
                else {
                    return Vec::new();
                };
                let pid = std::str::from_utf8(&output[pid_start..pid_start + line_length])
                    .map_err(|error| error.to_string())
                    .and_then(|text| {
                        text.trim()
                            .parse::<c_int>()
                            .map_err(|error| error.to_string())
                    });
                let _ = ready_tx.send(pid);
                acknowledged = true;
                b"\n".to_vec()
            },
            move || {
                callback_count.fetch_add(1, Ordering::AcqRel);
                let _ = exit_tx.send(());
            },
        )
        .expect("PTY should start");

        let descendant_pid = ready_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("background descendant should report its PID")
            .expect("background descendant PID should be valid");
        let descendant = KillProcessOnDrop(descendant_pid);
        // Signal 0 only checks that the background descendant is still alive.
        // SAFETY: `descendant_pid` was just reported by the direct child.
        assert_eq!(unsafe { kill(descendant_pid, 0) }, 0);

        exit_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("direct-child exit should not wait for the descendant's PTY descriptor");
        assert!(matches!(
            exit_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(exit_count.load(Ordering::Acquire), 1);

        drop(exit_sender_guard);
        drop(terminal);
        drop(descendant);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn resize_preempts_saturated_write_and_preserves_input_order() {
        const HELPER_ENV: &str = "TMON_PTY_BACKPRESSURE_TEST_HELPER";
        const READY: &[u8] = b"TMON_BACKPRESSURE_READY";
        const WINCH: &[u8] = b"TMON_BACKPRESSURE_WINCH";
        const ORDERED: &[u8] = b"TMON_BACKPRESSURE_ORDERED";
        const WRITE_COUNT: usize = 300;
        const CHUNK_BYTES: usize = 16 * 1024;

        fn identifiable_write(index: usize) -> Vec<u8> {
            let mut bytes = vec![(index % 251) as u8; CHUNK_BYTES];
            bytes[..size_of::<u32>()].copy_from_slice(&(index as u32).to_le_bytes());
            bytes
        }

        if std::env::var_os(HELPER_ENV).as_deref() == Some(OsStr::new("1")) {
            let mut input = io::stdin().lock();
            let mut chunk = vec![0; CHUNK_BYTES];
            for index in 0..WRITE_COUNT {
                input.read_exact(&mut chunk).unwrap_or_else(|error| {
                    panic!("helper did not receive write {index}: {error}")
                });
                assert!(
                    chunk == identifiable_write(index),
                    "write {index} was reordered or corrupted"
                );
            }
            io::stdout()
                .write_all(ORDERED)
                .expect("helper should report ordered input");
            io::stdout()
                .flush()
                .expect("helper should flush its ordered-input marker");
            return;
        }

        let executable = std::env::current_exe().expect("test executable should exist");
        let script = concat!(
            "stty raw -echo\n",
            "resized=\n",
            "trap 'resized=1; printf TMON_BACKPRESSURE_WINCH' WINCH\n",
            "printf TMON_BACKPRESSURE_READY\n",
            "while [ -z \"$resized\" ]; do :; done\n",
            "exec \"$1\" \"$2\" --exact --nocapture\n",
        );
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (winch_tx, winch_rx) = mpsc::sync_channel(1);
        let (ordered_tx, ordered_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::sync_channel(1);
        let mut output = Vec::new();
        let mut ready_reported = false;
        let mut winch_reported = false;
        let mut ordered_reported = false;
        let terminal = Pty::spawn(
            SpawnConfig {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    script.to_string(),
                    "tmon-backpressure-probe".to_string(),
                    executable.to_string_lossy().into_owned(),
                    "pty::tests::resize_preempts_saturated_write_and_preserves_input_order"
                        .to_string(),
                ],
                working_directory: None,
                environment: vec![(HELPER_ENV.to_string(), "1".to_string())],
            },
            test_size(),
            move |bytes| {
                output.extend_from_slice(bytes);
                if !ready_reported && output.windows(READY.len()).any(|part| part == READY) {
                    ready_reported = true;
                    let _ = ready_tx.send(());
                }
                if !winch_reported && output.windows(WINCH.len()).any(|part| part == WINCH) {
                    winch_reported = true;
                    let _ = winch_tx.send(());
                }
                if !ordered_reported && output.windows(ORDERED.len()).any(|part| part == ORDERED) {
                    ordered_reported = true;
                    let _ = ordered_tx.send(());
                }
                Vec::new()
            },
            move || {
                let _ = exit_tx.send(());
            },
        )
        .expect("backpressure PTY should start");

        ready_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("child should stop reading after configuring its PTY");
        for index in 0..WRITE_COUNT {
            terminal
                .write_owned(identifiable_write(index))
                .unwrap_or_else(|error| panic!("write {index} should queue: {error}"));
        }
        terminal
            .resize(Size {
                cols: 100,
                ..test_size()
            })
            .expect("resize should be published outside the write backlog");

        winch_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("resize should preempt the saturated write");
        ordered_rx
            .recv_timeout(Duration::from_secs(8))
            .expect("queued input should retain byte order after backpressure clears");
        exit_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("ordering helper should exit after reading all input");
        drop(terminal);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn canonical_unicode_erase_uses_iutf8() {
        const READY: &[u8] = b"TMON_IUTF8_READY";
        const RESULT: &[u8] = b"TMON_IUTF8_RESULT:";

        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let (exit_tx, exit_rx) = mpsc::channel();
        let mut output = Vec::new();
        let mut input_sent = false;
        let mut result_sent = false;
        let terminal = Pty::spawn(
            SpawnConfig {
                program: "/bin/sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    concat!(
                        "stty -echo erase '^?'\n",
                        "printf TMON_IUTF8_READY\n",
                        "IFS= read -r line\n",
                        "printf 'TMON_IUTF8_RESULT:%s\\n' \"$line\"\n",
                        "read confirmation\n",
                    )
                    .to_string(),
                ],
                working_directory: None,
                environment: Vec::new(),
            },
            test_size(),
            move |bytes| {
                output.extend_from_slice(bytes);
                let mut reply = Vec::new();
                if !input_sent && output.windows(READY.len()).any(|part| part == READY) {
                    input_sent = true;
                    // Canonical erase should remove the complete two-byte `é`,
                    // leaving only `X` for the shell's read.
                    reply.extend_from_slice(&[0xc3, 0xa9, 0x7f, b'X', b'\n']);
                }

                if !result_sent
                    && let Some(marker) =
                        output.windows(RESULT.len()).position(|part| part == RESULT)
                {
                    let value_start = marker + RESULT.len();
                    if let Some(line_length) =
                        output[value_start..].iter().position(|byte| *byte == b'\n')
                    {
                        let mut value = output[value_start..value_start + line_length].to_vec();
                        if value.last() == Some(&b'\r') {
                            value.pop();
                        }
                        let _ = result_tx.send(value);
                        result_sent = true;
                        reply.push(b'\n');
                    }
                }
                reply
            },
            move || {
                let _ = exit_tx.send(());
            },
        )
        .expect("PTY should start");

        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("child should report the canonical input"),
            b"X"
        );
        exit_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("child should exit after the result acknowledgment");
        drop(terminal);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum SignalObservation {
        Ready,
        Survived,
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn child_resets_inherited_ignored_signals() {
        const HELPER_ENV: &str = "TMON_SIGNAL_RESET_TEST_HELPER";
        if std::env::var_os(HELPER_ENV).as_deref() != Some(OsStr::new("1")) {
            let status =
                Command::new(std::env::current_exe().expect("test executable should exist"))
                    .arg("pty::tests::child_resets_inherited_ignored_signals")
                    .arg("--exact")
                    .arg("--nocapture")
                    .env(HELPER_ENV, "1")
                    .status()
                    .expect("isolated signal test should start");
            assert!(status.success(), "isolated signal test failed: {status}");
            return;
        }

        const READY: &[u8] = b"TMON_SIGNAL_READY";
        const SURVIVED: &[u8] = b"TMON_SIGNAL_SURVIVED";
        let (observation_tx, observation_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();
        let mut output = Vec::new();
        let mut ready_acknowledged = false;
        let mut survived_acknowledged = false;
        let config = SpawnConfig {
            program: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                concat!(
                    "printf TMON_SIGNAL_READY\n",
                    "read ready\n",
                    "kill -TERM $$\n",
                    "printf TMON_SIGNAL_SURVIVED\n",
                    "read survived\n",
                )
                .to_string(),
            ],
            working_directory: None,
            environment: Vec::new(),
        };

        // This process is an isolated test helper, so temporarily changing its
        // process-wide disposition cannot interfere with the main test runner.
        // SAFETY: SIG_IGN is the POSIX sentinel value, and the exact prior
        // disposition is restored immediately after forkpty returns.
        let previous = unsafe { signal(SIGTERM, SIGNAL_IGNORE) };
        assert_ne!(previous, SIGNAL_ERROR, "ignoring SIGTERM should succeed");
        let terminal = Pty::spawn(
            config,
            test_size(),
            move |bytes| {
                output.extend_from_slice(bytes);
                if !ready_acknowledged && output.windows(READY.len()).any(|part| part == READY) {
                    ready_acknowledged = true;
                    let _ = observation_tx.send(SignalObservation::Ready);
                    return b"\n".to_vec();
                }
                if !survived_acknowledged
                    && output.windows(SURVIVED.len()).any(|part| part == SURVIVED)
                {
                    survived_acknowledged = true;
                    let _ = observation_tx.send(SignalObservation::Survived);
                    return b"\n".to_vec();
                }
                Vec::new()
            },
            move || {
                let _ = exit_tx.send(());
            },
        );
        // SAFETY: `previous` is the handler returned by the successful signal call above.
        let restore_result = unsafe { signal(SIGTERM, previous) };
        assert_ne!(
            restore_result, SIGNAL_ERROR,
            "restoring SIGTERM should succeed"
        );
        let terminal = terminal.expect("PTY should start");

        assert_eq!(
            observation_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("child should reach the pre-signal handshake"),
            SignalObservation::Ready
        );
        exit_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("SIGTERM should terminate the child promptly");
        assert_ne!(
            observation_rx.try_recv(),
            Ok(SignalObservation::Survived),
            "the child inherited SIG_IGN instead of resetting SIGTERM"
        );
        drop(terminal);
    }
}
