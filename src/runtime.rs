//! Tokio lives on its own runtime, separate from gpui.
//!
//! gpui has its own executors but no IO reactor, so anything that talks to the
//! network (reqwest, axum, eventsource) runs on this runtime. UI code awaits
//! the result through [`spawn`] and then applies it on the gpui side.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::task::{JoinError, JoinHandle};

// `OnceLock` = a global that's created the first time it's used and then
// reused forever. Both the runtime and the HTTP client are built once.
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static HTTP: OnceLock<reqwest::Client> = OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            // Two threads is plenty: this app is waiting on the network, not crunching
            // numbers. (Zed itself uses a single worker thread for the same job.)
            .worker_threads(2)
            .thread_name("mailbox-io")
            .enable_all()
            .build()
            .expect("Failed to start tokio runtime")
    })
}

/// One shared HTTP client so connections (and TLS sessions) get reused
/// instead of doing a fresh handshake on every request.
// Why share one client? Creating a `reqwest::Client` per request (what the
// old code did) means a brand new TCP connection + TLS handshake every time,
// which is slow and CPU-heavy. A shared client keeps connections open and
// reuses them. `Client` is cheap to share: internally it's an `Arc`.
pub fn http() -> &'static reqwest::Client {
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client")
    })
}

/// Client for fetching URLs that came from untrusted content (images in
/// emails). It only connects to public internet addresses, so an email can't
/// make the app poke at localhost, the LAN or cloud metadata endpoints.
/// Hostnames are filtered at DNS resolution (which also covers redirects and
/// DNS rebinding); literal IPs in URLs are checked with `is_public_url`, both
/// up front and on every redirect hop.
pub fn http_public() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .dns_resolver(std::sync::Arc::new(PublicOnlyResolver))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if is_public_url(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error("redirect to a non-public address")
                }
            }))
            .build()
            .expect("Failed to build public HTTP client")
    })
}

/// http(s) URL whose host is a name (checked later, at DNS time) or a public IP.
pub fn is_public_url(url: &reqwest::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(domain)) => !domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => is_public_ip(ip.into()),
        None => false,
    }
}

fn is_public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(v4.into());
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00 // unique local
                || (first & 0xffc0) == 0xfe80) // link local
        }
    }
}

struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|addr| is_public_ip(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{} has no public address", name.as_str()).into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Client for long-lived streams (SSE). No overall timeout, or the stream
/// would be cut off after 30 seconds.
pub fn http_streaming() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to build streaming HTTP client")
}

/// Run a future on the tokio runtime. The returned handle aborts the tokio
/// task when dropped, so dropping the gpui `Task` that awaits it cancels the
/// network work too.
// Example of how UI code uses this:
//
//     let io = crate::runtime::spawn(async move { fetch_something().await });
//     cx.spawn(async move |this, cx| {
//         let result = io.await;            // waits without blocking the UI
//         this.update(cx, |view, cx| { ... apply result ... });
//     });
//
// The work runs on tokio's threads; only the final `update` touches the UI.
// The `Send + 'static` bounds are required because the future moves to
// another thread: it can't borrow anything from the caller and everything
// inside it must be safe to send between threads.
pub fn spawn<F>(future: F) -> AbortOnDrop<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    AbortOnDrop(runtime().spawn(future))
}

// Wrapper around tokio's `JoinHandle`. A plain `JoinHandle` does NOT stop the
// task when dropped (the task keeps running in the background). We want the
// opposite: when the UI no longer cares about a request (user clicked a
// different email, switched account, closed the view), dropping the handle
// should cancel the network work too. `abort()` in `Drop` does exactly that.
pub struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

// Lets you `.await` an `AbortOnDrop` directly. It just forwards to the inner
// `JoinHandle`. The output is `Result<T, JoinError>`: `Err` means the task
// panicked or was aborted.
impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_urls_pass() {
        let ok = |u: &str| is_public_url(&reqwest::Url::parse(u).unwrap());
        assert!(ok("https://example.com/a.png"));
        assert!(ok("http://93.184.216.34/a.png"));
        assert!(!ok("http://localhost:8080/"));
        assert!(!ok("http://127.0.0.1/"));
        assert!(!ok("http://192.168.1.1/"));
        assert!(!ok("http://10.0.0.5/"));
        assert!(!ok("http://169.254.169.254/latest/meta-data"));
        assert!(!ok("http://[::1]/"));
        assert!(!ok("http://[::ffff:127.0.0.1]/"));
        assert!(!ok("http://[fd00::1]/"));
        assert!(!ok("file:///etc/passwd"));
    }
}
