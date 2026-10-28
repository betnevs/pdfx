//! A minimal local HTTP/1.1 server for update-check tests.
//!
//! The update check is the only feature that touches the network, so its
//! command-boundary tests exercise a real TCP round trip. Each server answers
//! every accepted connection with the same scripted response and counts the
//! connections it saw, which is how the tests prove nothing connects unless
//! the check is explicitly invoked.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// What the server writes once it has read a request head.
#[derive(Clone)]
pub enum Response {
    /// A JSON body with the given status code.
    Status { code: u16, body: String },
    /// A 302 redirect to another address.
    Redirect { location: RedirectTarget },
    /// Accept the connection and stall without writing a response.
    Stall(Duration),
}

/// Destination of a [`Response::Redirect`].
#[derive(Clone)]
pub enum RedirectTarget {
    Fixed(String),
    /// Redirect to `/latest.json` on this same server, producing a loop.
    SelfAddress,
}

pub struct TestServer {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    pub fn start(response: Response) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the test HTTP server");
        listener
            .set_nonblocking(true)
            .expect("make the test server non-blocking");
        let address = listener.local_addr().expect("test server address");
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let handle = {
            let requests = Arc::clone(&requests);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            requests.fetch_add(1, Ordering::Relaxed);
                            respond(stream, &response, address);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
        };

        Self {
            address,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn respond(mut stream: TcpStream, response: &Response, address: SocketAddr) {
    // On macOS an accepted socket inherits the listener's non-blocking flag, so
    // the read below would race the client's request and spuriously close the
    // connection. Accepted connections are handled one at a time and must block.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if !read_request_head(&mut stream) {
        return;
    }

    match response {
        Response::Status { code, body } => {
            let head = format!(
                "HTTP/1.1 {code} {}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                reason(*code),
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
        Response::Redirect { location } => {
            let location = match location {
                RedirectTarget::Fixed(location) => location.clone(),
                RedirectTarget::SelfAddress => format!("http://{address}/latest.json"),
            };
            let head = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
        }
        Response::Stall(delay) => {
            thread::sleep(*delay);
        }
    }
    let _ = stream.flush();
    // Signal the end of the body with a FIN. Do not shutdown reads: a
    // read-shutdown with unread bytes can turn the close into a TCP reset and
    // truncate the response before the client parses it.
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Response",
    }
}

fn read_request_head(stream: &mut TcpStream) -> bool {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 512];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return false,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                    return true;
                }
                if buffer.len() > 16 * 1024 {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}
