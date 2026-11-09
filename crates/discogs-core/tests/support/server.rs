//! A tiny HTTP/1.1 server on 127.0.0.1 for the network tests: one
//! connection per request (`Connection: close`), canned replies per request
//! target, a count of requests per target.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// What a request gets.
#[derive(Clone, Debug)]
pub enum Reply {
    /// 200 with this body.
    Body(Vec<u8>),
    /// 200 announcing the whole body's length, then only its first `after`
    /// bytes, then the connection closes.
    Drop { body: Vec<u8>, after: usize },
    /// 302 to this location.
    Redirect(String),
    /// This status, empty body.
    Status(u16),
}

#[derive(Default)]
struct Inner {
    /// Target → replies; the n-th request gets the n-th (the last repeats).
    routes: HashMap<String, Vec<Reply>>,
    hits: HashMap<String, usize>,
    user_agents: Vec<String>,
}

#[derive(Clone)]
pub struct Server {
    pub base: String,
    inner: Arc<Mutex<Inner>>,
}

impl Server {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let inner = Arc::new(Mutex::new(Inner::default()));
        let shared = inner.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = shared.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream, &shared);
                });
            }
        });
        Self { base, inner }
    }

    /// Serves `replies` at `target` (`/path?query`, as sent).
    pub fn route(&self, target: &str, replies: Vec<Reply>) -> &Self {
        self.inner.lock().unwrap().routes.insert(target.to_string(), replies);
        self
    }

    /// The full URL of a target.
    pub fn url(&self, target: &str) -> String {
        format!("{}{}", self.base.trim_end_matches('/'), target)
    }

    pub fn hits(&self, target: &str) -> usize {
        self.inner.lock().unwrap().hits.get(target).copied().unwrap_or(0)
    }

    pub fn user_agents(&self) -> Vec<String> {
        self.inner.lock().unwrap().user_agents.clone()
    }
}

fn serve(mut stream: TcpStream, shared: &Mutex<Inner>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let mut ua = String::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" {
            break;
        }
        if let Some((name, value)) = h.split_once(':') {
            if name.eq_ignore_ascii_case("user-agent") {
                ua = value.trim().to_string();
            }
        }
    }
    let reply = {
        let mut inner = shared.lock().unwrap();
        inner.user_agents.push(ua);
        let n = *inner.hits.entry(target.clone()).and_modify(|n| *n += 1).or_insert(1);
        inner
            .routes
            .get(&target)
            .map(|r| r[(n - 1).min(r.len() - 1)].clone())
            .unwrap_or(Reply::Status(404))
    };
    let head = |status: &str, len: usize, extra: &str| {
        format!("HTTP/1.1 {status}\r\nContent-Length: {len}\r\nConnection: close\r\n{extra}\r\n")
    };
    match reply {
        Reply::Body(body) => {
            stream.write_all(head("200 OK", body.len(), "").as_bytes())?;
            stream.write_all(&body)?;
        }
        Reply::Drop { body, after } => {
            stream.write_all(head("200 OK", body.len(), "").as_bytes())?;
            stream.write_all(&body[..after])?;
            stream.flush()?;
        }
        Reply::Redirect(to) => {
            stream.write_all(head("302 Found", 0, &format!("Location: {to}\r\n")).as_bytes())?;
        }
        Reply::Status(code) => {
            stream.write_all(head(&format!("{code} Status"), 0, "").as_bytes())?;
        }
    }
    stream.flush()?;
    stream.shutdown(Shutdown::Both)
}
