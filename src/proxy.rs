use anyhow::{Context, Result};
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
    thread,
    time::Duration,
};

use crate::cache::download_page_curl;
use crate::types::Page;

#[derive(Clone)]
pub struct CachedPageTarget {
    pub page: Page,
    pub path: PathBuf,
}

pub struct LocalPageProxy {
    pub base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl LocalPageProxy {
    pub fn start(targets: Vec<CachedPageTarget>) -> Result<Self> {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).context("failed to bind local page cache proxy")?;
        let addr = listener
            .local_addr()
            .context("failed to read local proxy address")?;
        listener
            .set_nonblocking(true)
            .context("failed to configure local proxy socket")?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_signal = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stop_signal.load(AtomicOrdering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if let Err(err) = handle_proxy_request(&mut stream, &targets) {
                            if is_benign_proxy_error(&err) {
                                continue;
                            }
                            if let Err(write_err) =
                                write_http_error(&mut stream, 500, "proxy error")
                            {
                                if !is_benign_proxy_error(&write_err) {
                                    eprintln!(
                                        "Local cache proxy: failed to write error response: {write_err}"
                                    );
                                }
                            }
                            eprintln!("Local cache proxy request failed: {err}");
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(err) => {
                        eprintln!("Local cache proxy accept failed: {err}");
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        });

        Ok(Self {
            base_url: format!("http://127.0.0.1:{}", addr.port()),
            stop,
            handle: Some(handle),
        })
    }

    pub fn page_url(&self, idx: usize) -> String {
        format!("{}/{}", self.base_url, idx)
    }

    pub fn shutdown(&mut self) {
        self.stop.store(true, AtomicOrdering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LocalPageProxy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn handle_proxy_request(stream: &mut TcpStream, targets: &[CachedPageTarget]) -> Result<()> {
    use std::fs;
    let mut reader = BufReader::new(stream.try_clone().context("failed to clone proxy stream")?);
    let mut request_line = String::new();
    let bytes_read = reader
        .read_line(&mut request_line)
        .context("failed to read proxy request")?;
    if bytes_read == 0 {
        return Ok(());
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    if method != "GET" && method != "HEAD" {
        write_http_error(stream, 405, "method not allowed")?;
        return Ok(());
    }

    let idx = path
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or_default()
        .parse::<usize>()
        .ok();
    let Some(idx) = idx else {
        write_http_error(stream, 404, "not found")?;
        return Ok(());
    };

    let Some(target) = targets.get(idx) else {
        write_http_error(stream, 404, "not found")?;
        return Ok(());
    };

    if !target.path.exists()
        && let Err(err) = download_page_curl(&target.page, &target.path)
    {
        write_http_error(stream, 502, "cache fetch failed")?;
        return Err(err.context(format!(
            "failed to fetch page {} for proxy",
            target.page.url
        )));
    }

    let data = fs::read(&target.path)
        .with_context(|| format!("failed to read cached file {}", target.path.display()))?;
    if method == "HEAD" {
        write_http_head(stream, data.len(), mime_type_for_path(&target.path))?;
    } else {
        write_http_ok(stream, &data, mime_type_for_path(&target.path))?;
    }
    Ok(())
}

pub fn write_http_ok(stream: &mut TcpStream, body: &[u8], content_type: &str) -> Result<()> {
    if let Err(err) = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
        body.len(),
        content_type
    ) {
        if is_benign_disconnect(&err) {
            return Ok(());
        }
        return Err(err).context("failed to write proxy headers");
    }
    if let Err(err) = stream.write_all(body) {
        if is_benign_disconnect(&err) {
            return Ok(());
        }
        return Err(err).context("failed to write proxy response body");
    }
    Ok(())
}

pub fn write_http_head(
    stream: &mut TcpStream,
    content_length: usize,
    content_type: &str,
) -> Result<()> {
    if let Err(err) = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
        content_length, content_type
    ) {
        if is_benign_disconnect(&err) {
            return Ok(());
        }
        return Err(err).context("failed to write proxy head response");
    }
    Ok(())
}

pub fn write_http_error(stream: &mut TcpStream, status: u16, message: &str) -> Result<()> {
    let body = message.as_bytes();
    let reason = match status {
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        _ => "Internal Server Error",
    };
    if let Err(err) = write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n{}",
        status,
        reason,
        body.len(),
        message
    ) {
        if is_benign_disconnect(&err) {
            return Ok(());
        }
        return Err(err).context("failed to write proxy error response");
    }
    Ok(())
}

pub fn is_benign_disconnect(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::UnexpectedEof
    )
}

pub fn is_benign_proxy_error(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(is_benign_disconnect)
}

pub fn mime_type_for_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("gif") => "image/gif",
        _ => "application/octet-stream",
    }
}
