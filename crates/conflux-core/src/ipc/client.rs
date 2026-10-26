//! Synchronous IPC client, shared by the CLI and the GUI.
//!
//! It stays runtime-free: it opens the Unix socket with `std`, writes one JSON
//! request line, and reads one JSON response line.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use super::{Request, Response};

/// Send a request to the daemon listening on `socket` and return its response.
///
/// `timeout` bounds each read and write; `None` waits as long as the request
/// takes (a sync can run for minutes). A daemon that is not running shows as
/// [`io::ErrorKind::NotFound`] (no socket) or [`io::ErrorKind::ConnectionRefused`]
/// (a stale one), see [`is_not_running`]; a socket the caller may not open as
/// [`io::ErrorKind::PermissionDenied`].
pub fn send(socket: &Path, request: &Request, timeout: Option<Duration>) -> io::Result<Response> {
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;

    let mut line = serde_json::to_string(request)?;
    line.push('\n');
    (&stream).write_all(line.as_bytes())?;

    let mut reader = BufReader::new(&stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    if response.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "daemon closed the connection without responding",
        ));
    }
    serde_json::from_str(response.trim()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Whether an error from [`send`] means no daemon is listening on the socket.
pub fn is_not_running(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn missing_socket_reads_as_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let err = send(&dir.path().join("none.sock"), &Request::Status, None).unwrap_err();
        assert!(is_not_running(&err));
    }

    #[test]
    fn round_trips_one_request_and_response() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            assert!(matches!(
                serde_json::from_str::<Request>(line.trim()).unwrap(),
                Request::Info
            ));
            let reply = serde_json::to_string(&Response::Error("nope".into())).unwrap();
            (&stream)
                .write_all(format!("{reply}\n").as_bytes())
                .unwrap();
        });
        let response = send(&path, &Request::Info, Some(Duration::from_secs(5))).unwrap();
        server.join().unwrap();
        assert!(matches!(response, Response::Error(m) if m == "nope"));
    }

    #[test]
    fn old_status_without_new_fields_still_parses() {
        let json = r#"{"Status":[{"label":"r:p","remote":"r","root":"/x","trigger":"watch",
            "last_run":null,"last_summary":null,"last_error":null}]}"#;
        let Response::Status(groups) = serde_json::from_str(json).unwrap() else {
            panic!("expected a status response");
        };
        assert_eq!(groups[0].id, None);
        assert!(!groups[0].running);
    }
}
