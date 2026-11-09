//! HTTPS input (feature `http`): a blocking client over `reqwest` and
//! `rustls` (ring, Mozilla roots built in). `reqwest` runs its own runtime
//! on a thread of its own; nothing async reaches the caller.
//!
//! data.discogs.com ignores `Range`, so a transfer cannot resume: a body
//! that breaks off is an [`Error::Network`] and the caller starts the file
//! over ([`crate::run`] does, a few times).

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::dumps::{self, checksum_file, download_url, dumps_in_listing, listing_url, parse_checksums};
use crate::{Error, Result};

/// The User-Agent sent with every request: `discogs-rs/<version>`.
pub const USER_AGENT: &str = concat!("discogs-rs/", env!("CARGO_PKG_VERSION"));

/// Whether a source is a URL (`http://` or `https://`) rather than a path.
pub fn is_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

/// How to reach Discogs.
#[derive(Clone, Debug)]
pub struct HttpOptions {
    /// The data site; [`dumps::DISCOGS_DATA`] by default.
    pub base_url: String,
    /// Extra whole tries after a failed one (a dropped connection, a 5xx,
    /// a 429). Default 2.
    pub retries: u32,
    /// The longest wait for a connection, a response or the next bytes of a
    /// body. Default 120 s.
    pub timeout: Duration,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            base_url: dumps::DISCOGS_DATA.to_string(),
            retries: 2,
            timeout: Duration::from_secs(120),
        }
    }
}

/// A transport failure: a body that broke off, a reset, a timeout, a 5xx.
/// Worth retrying, unlike a 404.
#[derive(Debug)]
pub(crate) struct NetworkFailure(pub String);

impl std::fmt::Display for NetworkFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NetworkFailure {}

/// The error an [`io::Error`] stands for: [`Error::Network`] when it came
/// from a [`Body`], else [`Error::Io`].
pub(crate) fn classify(e: io::Error) -> Error {
    match e.get_ref().and_then(|r| r.downcast_ref::<NetworkFailure>()) {
        Some(n) => Error::Network(n.0.clone()),
        None => Error::Io(e),
    }
}

fn network(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(NetworkFailure(e.to_string()))
}

/// A blocking HTTP client for the data site.
#[derive(Clone)]
pub struct Http {
    client: reqwest::blocking::Client,
    options: HttpOptions,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http").field("options", &self.options).finish()
    }
}

fn tls_config() -> rustls::ClientConfig {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports TLS 1.2 and 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

impl Http {
    /// A client. Redirects (the data site sends files to its bucket) are
    /// followed, up to 10.
    pub fn new(options: HttpOptions) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .tls_backend_preconfigured(tls_config())
            .connect_timeout(options.timeout)
            // Per call: a request, then each read of the body.
            .timeout(options.timeout)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| Error::Http(format!("client: {e}")))?;
        Ok(Self { client, options })
    }

    /// The options it was built with.
    pub fn options(&self) -> &HttpOptions {
        &self.options
    }

    /// Opens `url` for reading; a non-2xx answer is an error
    /// ([`Error::Network`] for a 5xx or 429, else [`Error::Http`]).
    pub fn open(&self, url: &str) -> Result<Body> {
        let started = Instant::now();
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|e| Error::Network(format!("{url}: {}", chain(&e))))?;
        let status = response.status();
        if !status.is_success() {
            let msg = format!("{url}: HTTP {status}");
            return Err(if status.is_server_error() || status.as_u16() == 429 {
                Error::Network(msg)
            } else {
                Error::Http(msg)
            });
        }
        Ok(Body {
            length: response.content_length(),
            received: 0,
            url: url.to_string(),
            response,
            waited: started.elapsed(),
            clock: None,
        })
    }

    /// `url`'s body as text, retried like a dump file.
    pub fn get_text(&self, url: &str) -> Result<String> {
        let mut tries = 0;
        loop {
            let attempt = self.open(url).and_then(|mut body| {
                let mut text = String::new();
                body.read_to_string(&mut text).map_err(classify)?;
                Ok(text)
            });
            match attempt {
                Err(Error::Network(_)) if tries < self.options.retries => tries += 1,
                other => return other,
            }
        }
    }

    /// A dump's `CHECKSUM.txt`: file name → sha256.
    pub fn checksums(&self, dump: &str) -> Result<BTreeMap<String, String>> {
        if !dumps::is_dump(dump) {
            return Err(Error::Checksum(format!("{dump:?} names no dump (YYYYMMDD)")));
        }
        let url = download_url(&self.options.base_url, dump, &checksum_file(dump));
        let sums = parse_checksums(&self.get_text(&url)?);
        if sums.is_empty() {
            return Err(Error::Checksum(format!("{url}: no checksums in it")));
        }
        Ok(sums)
    }

    /// The checksums of the `CHECKSUM.txt` at `url`.
    pub fn checksums_at(&self, url: &str) -> Result<BTreeMap<String, String>> {
        Ok(parse_checksums(&self.get_text(url)?))
    }

    /// The complete dumps a year's listing names, oldest first.
    pub fn dumps_in_year(&self, year: i32) -> Result<Vec<String>> {
        Ok(dumps_in_listing(
            &self.get_text(&listing_url(&self.options.base_url, year))?,
        ))
    }

    /// The newest complete dump (its `CHECKSUM.txt` listed), as of today.
    pub fn latest_dump(&self) -> Result<String> {
        let (year, month) = dumps::today_utc();
        self.latest_dump_on(year, month)
    }

    /// The newest complete dump as of `year`/`month` (1–12): the year's
    /// listing, and the year before's in January or when this year lists
    /// none yet (January's dump came on the 15th in 2026).
    pub fn latest_dump_on(&self, year: i32, month: u32) -> Result<String> {
        let mut dumps = self.dumps_in_year(year)?;
        if month == 1 || dumps.is_empty() {
            dumps.extend(self.dumps_in_year(year - 1)?);
            dumps.sort();
        }
        dumps
            .pop()
            .ok_or_else(|| Error::Http(format!("no Discogs dump listed for {}–{year}", year - 1)))
    }
}

/// An error and its sources, on one line (reqwest's own message is terse).
fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut next = e.source();
    while let Some(c) = next {
        s.push_str(": ");
        s.push_str(&c.to_string());
        next = c.source();
    }
    s
}

/// A response body being read. Its errors are [`Error::Network`] ones
/// (see [`classify`]), and so is an end before the announced length.
pub struct Body {
    response: reqwest::blocking::Response,
    url: String,
    length: Option<u64>,
    received: u64,
    waited: Duration,
    clock: Option<Arc<Mutex<Duration>>>,
}

impl Body {
    /// The announced length (`Content-Length`), if any.
    pub fn content_length(&self) -> Option<u64> {
        self.length
    }

    /// Adds the time spent on the request and in every read to `clock`.
    pub fn timed(mut self, clock: Arc<Mutex<Duration>>) -> Self {
        *clock.lock().unwrap_or_else(|p| p.into_inner()) += self.waited;
        self.clock = Some(clock);
        self
    }
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let t = Instant::now();
        let r = self.response.read(buf);
        if let Some(clock) = &self.clock {
            *clock.lock().unwrap_or_else(|p| p.into_inner()) += t.elapsed();
        }
        match r {
            Ok(0) if !buf.is_empty() => match self.length {
                Some(len) if self.received < len => Err(network(format!(
                    "{}: the connection closed after {} of {len} bytes",
                    self.url, self.received
                ))),
                _ => Ok(0),
            },
            Ok(n) => {
                self.received += n as u64;
                Ok(n)
            }
            Err(e) => Err(network(format!(
                "{}: after {} bytes: {}",
                self.url,
                self.received,
                chain(&e)
            ))),
        }
    }
}
