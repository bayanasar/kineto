//! Just enough HTTP/1.1 for MCP's Streamable HTTP transport and OAuth: one
//! request per connection, a body of known length, and an answer that
//! closes the connection.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// An answer may wait for the director; a tool call waits at most this
/// long before it answers anyway.
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);
const ACCEPT_INTERVAL: Duration = Duration::from_millis(100);

/// One HTTP request, as far as this server reads one.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// The path, without its query.
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The value of header `name`, given in lower case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    /// The query's fields, decoded.
    pub fn query_fields(&self) -> Vec<(String, String)> {
        form_fields(&self.query)
    }

    /// The body's fields, when it is a form.
    pub fn form(&self) -> Vec<(String, String)> {
        std::str::from_utf8(&self.body)
            .map(form_fields)
            .unwrap_or_default()
    }
}

/// `name`'s value among `fields`, when it is given exactly once.
pub fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let mut values = fields.iter().filter(|(key, _)| key == name);
    match (values.next(), values.next()) {
        (Some((_, value)), None) => Some(value.as_str()),
        _ => None,
    }
}

/// The fields of `text`, as in `a=1&b=two%20words`.
pub fn form_fields(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect()
}

/// `text` with `+` as a space and `%XX` as the byte it names; a sequence
/// that is not UTF-8 becomes U+FFFD.
fn decode(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, after)) = rest.split_first() {
        match byte {
            b'+' => {
                bytes.push(b' ');
                rest = after;
            }
            b'%' if after.len() >= 2 => {
                match std::str::from_utf8(&after[..2])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(decoded) => {
                        bytes.push(decoded);
                        rest = &after[2..];
                    }
                    None => {
                        bytes.push(byte);
                        rest = after;
                    }
                }
            }
            _ => {
                bytes.push(byte);
                rest = after;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `text` as a query value: every byte but the unreserved ones as `%XX`.
pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// An answer.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub const fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "application/json".to_owned())],
            body: value.to_string().into_bytes(),
        }
    }

    pub fn html(status: u16, page: String) -> Self {
        Self {
            status,
            headers: vec![
                ("Content-Type", "text/html; charset=utf-8".to_owned()),
                (
                    "Content-Security-Policy",
                    "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'"
                        .to_owned(),
                ),
                ("X-Frame-Options", "DENY".to_owned()),
            ],
            body: page.into_bytes(),
        }
    }

    pub fn redirect(location: String) -> Self {
        Self {
            status: 302,
            headers: vec![("Location", location)],
            body: Vec::new(),
        }
    }

    #[must_use]
    pub fn with(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }
}

const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        _ => "Error",
    }
}

/// Why a request was not read.
enum Unread {
    TooLarge,
    LengthRequired,
    Malformed,
}

fn read_request(stream: &TcpStream) -> Result<Request, Unread> {
    let mut reader = BufReader::new(stream.take((MAX_HEAD_BYTES + MAX_BODY_BYTES) as u64));
    let mut head = 0;
    let mut line = String::new();
    let mut next_line = |line: &mut String| -> Result<(), Unread> {
        line.clear();
        let read = reader.read_line(line).map_err(|_| Unread::Malformed)?;
        head += read;
        if read == 0 {
            Err(Unread::Malformed)
        } else if head > MAX_HEAD_BYTES {
            Err(Unread::TooLarge)
        } else {
            Ok(())
        }
    };
    next_line(&mut line)?;
    let mut words = line.split_whitespace();
    let (Some(method), Some(target), Some(_)) = (words.next(), words.next(), words.next()) else {
        return Err(Unread::Malformed);
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (method, path, query) = (method.to_owned(), path.to_owned(), query.to_owned());
    let mut headers = Vec::new();
    loop {
        next_line(&mut line)?;
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').ok_or(Unread::Malformed)?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    let mut request = Request {
        method,
        path,
        query,
        headers,
        body: Vec::new(),
    };
    if request.header("transfer-encoding").is_some() {
        return Err(Unread::LengthRequired);
    }
    let length = match request.header("content-length") {
        Some(length) => length.parse::<usize>().map_err(|_| Unread::Malformed)?,
        None => 0,
    };
    if length > MAX_BODY_BYTES {
        return Err(Unread::TooLarge);
    }
    request.body = vec![0; length];
    reader
        .read_exact(&mut request.body)
        .map_err(|_| Unread::Malformed)?;
    Ok(request)
}

/// What answers the requests one listener takes.
pub trait Answer: Send + Sync + 'static {
    fn answer(&self, request: &Request) -> Response;
}

fn serve(answer: &dyn Answer, mut stream: TcpStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let response = match read_request(&stream) {
        Ok(request) => answer.answer(&request),
        Err(Unread::TooLarge) => Response::empty(413),
        Err(Unread::LengthRequired) => Response::empty(411),
        Err(Unread::Malformed) => Response::empty(400),
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nConnection: close\r\nCache-Control: no-store\r\n",
        response.status,
        reason(response.status)
    );
    for (name, value) in &response.headers {
        // A header never spans lines, whatever a value held.
        let value: String = value.chars().filter(|c| !c.is_control()).collect();
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", response.body.len()));
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

/// A listener on `127.0.0.1`, answering until it is dropped.
pub struct Listener {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Listener {
    /// Listen on `127.0.0.1:port` (any free port for 0), answering each
    /// request on a thread of its own.
    pub fn start(port: u16, name: &str, answer: Arc<dyn Answer>) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let request_name = format!("{name}-request");
        let thread = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let answer = Arc::clone(&answer);
                            let _ = thread::Builder::new()
                                .name(request_name.clone())
                                .spawn(move || serve(answer.as_ref(), stream));
                        }
                        Err(_) => thread::sleep(ACCEPT_INTERVAL),
                    }
                }
            })?;
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }

    pub const fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_is_decoded_and_a_value_encoded() {
        let fields = form_fields(
            "grant_type=authorization_code&redirect_uri=https%3A%2F%2Fchatgpt.com%2Fa&note=two+words&bad=%zz",
        );
        assert_eq!(field(&fields, "grant_type"), Some("authorization_code"));
        assert_eq!(
            field(&fields, "redirect_uri"),
            Some("https://chatgpt.com/a")
        );
        assert_eq!(field(&fields, "note"), Some("two words"));
        assert_eq!(field(&fields, "bad"), Some("%zz"));
        assert_eq!(field(&form_fields("a=1&a=2"), "a"), None, "given twice");
        assert_eq!(encode("a b/c?d=é"), "a%20b%2Fc%3Fd%3D%C3%A9");
    }
}
