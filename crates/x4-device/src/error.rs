use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("The reader did not answer in time ({0})")]
    Timeout(String),

    /// An `ERR:<code>` line from the firmware.
    #[error("{}", describe_device_error(.0))]
    Device(String),

    #[error("Unexpected reply from the reader: {0}")]
    Protocol(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Serial port error: {0}")]
    Serial(#[from] serialport::Error),

    #[error("Not supported: {0}")]
    Unsupported(String),

    #[error("No reader connected")]
    NotConnected,
}

impl Error {
    /// True when the device answered `not_on_home` (USB transfer is gated on the home screen).
    pub fn is_not_on_home(&self) -> bool {
        matches!(self, Error::Device(code) if code == "not_on_home")
    }

    /// True when the serial port itself is gone (the reader went to sleep, rebooted or was
    /// unplugged), so the handle is dead and has to be opened again. Only unambiguous cases:
    /// a generic I/O error is not treated as a disconnect.
    pub fn is_disconnected(&self) -> bool {
        use std::io::ErrorKind as K;
        let gone = |k: K| matches!(k, K::BrokenPipe | K::NotConnected);
        match self {
            // ENXIO, ENODEV: what macOS and Linux return for a vanished tty.
            Error::Io(e) => gone(e.kind()) || matches!(e.raw_os_error(), Some(6 | 19)),
            Error::Serial(e) => match e.kind() {
                serialport::ErrorKind::NoDevice => true,
                serialport::ErrorKind::Io(k) => gone(k),
                _ => false,
            },
            _ => false,
        }
    }

    /// True when the port could not be found at all (as opposed to found but not answering).
    pub fn is_port_missing(&self) -> bool {
        match self {
            Error::Io(e) => {
                e.kind() == std::io::ErrorKind::NotFound
                    || matches!(e.raw_os_error(), Some(2 | 6 | 19))
            }
            Error::Serial(e) => match e.kind() {
                serialport::ErrorKind::NoDevice => true,
                serialport::ErrorKind::Io(k) => k == std::io::ErrorKind::NotFound,
                _ => false,
            },
            _ => false,
        }
    }
}

fn describe_device_error(code: &str) -> String {
    let (head, tail) = code.split_once(':').unwrap_or((code, ""));
    let text = match head {
        "not_on_home" => "Open the home screen on your reader: USB transfer only works there",
        "protected_path" => "The firmware protects this path (hidden or system files)",
        "not_found" => "File or folder not found",
        "target_exists" => "A file with this name already exists",
        "is_directory" => "This is a folder, not a file",
        "not_directory" => "This is a file, not a folder",
        "invalid_path" => "Invalid path",
        "path_too_long" => "Path too long (255 bytes max)",
        "mkdir_failed" | "mkdir_not_visible" => "Could not create the folder",
        "remove_failed" => "Could not delete",
        "rename_failed" => "Could not rename",
        "fopen" | "write" | "read" => "SD card error",
        "oom" => "The reader ran out of memory",
        "crc" => "Checksum mismatch: the file was damaged in transfer",
        "crc_missing" | "timeout" | "rx_overflow" => "The transfer was interrupted",
        "too_large" => "File too large",
        "unknown_cmd" | "cmd" => "The firmware does not support this command",
        _ => "",
    };
    match (text.is_empty(), tail.is_empty()) {
        (true, _) => format!("The reader returned an error: {code}"),
        (false, true) => text.to_string(),
        (false, false) => format!("{text} ({tail})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_a_dropped_port() {
        let io = |k| Error::Io(std::io::Error::from(k));
        assert!(io(std::io::ErrorKind::BrokenPipe).is_disconnected());
        assert!(Error::Io(std::io::Error::from_raw_os_error(6)).is_disconnected());
        assert!(Error::Serial(serialport::Error::new(
            serialport::ErrorKind::NoDevice,
            "gone"
        ))
        .is_disconnected());
        // A generic I/O error (EIO) or a timeout is not a disconnect.
        assert!(!Error::Io(std::io::Error::from_raw_os_error(5)).is_disconnected());
        assert!(!io(std::io::ErrorKind::TimedOut).is_disconnected());
        assert!(!Error::Device("not_on_home".into()).is_disconnected());
    }

    #[test]
    fn tells_a_missing_port_from_a_silent_one() {
        assert!(Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)).is_port_missing());
        assert!(!Error::Timeout("status".into()).is_port_missing());
        assert!(!Error::Device("not_on_home".into()).is_port_missing());
    }
}
