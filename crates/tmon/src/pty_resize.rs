use std::{io, sync::mpsc};

#[cfg(not(target_os = "windows"))]
pub(crate) fn writer_closed_error() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "tmon PTY is closed")
}

pub(crate) struct PendingResize<T> {
    pub(crate) size: T,
    completion: Option<mpsc::Sender<io::Result<()>>>,
}

pub(crate) fn await_resize_completion(
    publication: io::Result<()>,
    completed: mpsc::Receiver<io::Result<()>>,
    closed_error: impl FnOnce() -> io::Error,
) -> io::Result<()> {
    // An out-of-band resize can finish before a late wake observes channel
    // disconnection. Once published, its completion is authoritative.
    match completed.recv() {
        Ok(result) => result,
        Err(_) => match publication {
            Ok(()) => Err(closed_error()),
            Err(error) => Err(error),
        },
    }
}

impl<T> PendingResize<T> {
    #[cfg(test)]
    pub(crate) fn detached(size: T) -> Self {
        Self {
            size,
            completion: None,
        }
    }

    pub(crate) fn waiting(size: T) -> (Self, mpsc::Receiver<io::Result<()>>) {
        let (completion, completed) = mpsc::channel();
        (
            Self {
                size,
                completion: Some(completion),
            },
            completed,
        )
    }

    pub(crate) fn complete(self, result: io::Result<()>) {
        if let Some(completion) = self.completion {
            let _ = completion.send(result);
        }
    }
}
