use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{Delivery, ResponseContentEncoding, ResponseFraming, ScenarioPlan, WireEncoding};
use sha2::{Digest, Sha256};

// Full workspace runs can briefly starve the simulator thread while native jobs link or scan
// several test binaries in parallel. Keep the synthetic peer patient enough that scheduler
// delay is not misclassified as a Tally transport failure, while polling keeps cancellation
// responsive when a client disconnects before completing its request.
const ACCEPT_DEADLINE: Duration = Duration::from_secs(30);
const REQUEST_READ_DEADLINE: Duration = Duration::from_secs(30);
const REQUEST_READ_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_REQUEST_BYTES: usize = 128 * 1024;
pub const MAX_SEQUENCE_REQUESTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedRequest {
    pub method: String,
    pub path: String,
    pub bytes_received: usize,
    pub request_body_bytes: usize,
    pub request_body_sha256: String,
    pub request_processed: bool,
    pub cancelled: bool,
}

pub struct Simulator {
    address: SocketAddr,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<ObservedRequest>>>,
}

pub struct SequenceSimulator {
    address: SocketAddr,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<Vec<ObservedRequest>>>>,
}

impl Simulator {
    pub fn spawn(plan: ScenarioPlan) -> io::Result<Self> {
        let listener = bind_loopback_listener()?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        debug_assert!(address.ip().is_loopback());
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("tally-protocol-simulator".to_owned())
            .spawn(move || {
                let _ = ready_tx.send(());
                serve_once(listener, plan, worker_cancelled)
            })?;
        ready_rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| io::Error::other("simulator worker did not become ready"))?;
        Ok(Self {
            address,
            cancelled,
            worker: Some(worker),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(50));
    }

    pub fn finish(mut self) -> io::Result<ObservedRequest> {
        let worker = self
            .worker
            .take()
            .ok_or_else(|| io::Error::other("simulator worker already joined"))?;
        worker
            .join()
            .map_err(|_| io::Error::other("simulator worker panicked"))?
    }
}

impl SequenceSimulator {
    pub fn spawn(plans: Vec<ScenarioPlan>) -> io::Result<Self> {
        if plans.is_empty() || plans.len() > MAX_SEQUENCE_REQUESTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "simulator sequence request count is out of range",
            ));
        }
        let listener = bind_loopback_listener()?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        debug_assert!(address.ip().is_loopback());
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("tally-protocol-sequence-simulator".to_owned())
            .spawn(move || {
                let _ = ready_tx.send(());
                serve_sequence(listener, plans, worker_cancelled)
            })?;
        ready_rx
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| io::Error::other("simulator sequence worker did not become ready"))?;
        Ok(Self {
            address,
            cancelled,
            worker: Some(worker),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(50));
    }

    pub fn finish(mut self) -> io::Result<Vec<ObservedRequest>> {
        let worker = self
            .worker
            .take()
            .ok_or_else(|| io::Error::other("sequence simulator worker already joined"))?;
        worker
            .join()
            .map_err(|_| io::Error::other("sequence simulator worker panicked"))?
    }
}

fn bind_loopback_listener() -> io::Result<TcpListener> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
}

impl Drop for SequenceSimulator {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            if !worker.is_finished() {
                self.cancel();
            }
            let _ = worker.join();
        }
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            if !worker.is_finished() {
                self.cancel();
            }
            let _ = worker.join();
        }
    }
}

fn serve_once(
    listener: TcpListener,
    plan: ScenarioPlan,
    cancelled: Arc<AtomicBool>,
) -> io::Result<ObservedRequest> {
    serve_request(&listener, plan, &cancelled)
}

fn serve_sequence(
    listener: TcpListener,
    plans: Vec<ScenarioPlan>,
    cancelled: Arc<AtomicBool>,
) -> io::Result<Vec<ObservedRequest>> {
    let mut observed = Vec::with_capacity(plans.len());
    for plan in plans {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        observed.push(serve_request(&listener, plan, &cancelled)?);
    }
    Ok(observed)
}

fn serve_request(
    listener: &TcpListener,
    plan: ScenarioPlan,
    cancelled: &AtomicBool,
) -> io::Result<ObservedRequest> {
    let started = Instant::now();
    let (mut stream, request) = loop {
        let (mut stream, _) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if started.elapsed() >= ACCEPT_DEADLINE {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "simulator received no request",
                    ));
                }
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) => return Err(error),
        };
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(REQUEST_READ_POLL_INTERVAL))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let remaining_read_deadline = REQUEST_READ_DEADLINE
            .checked_sub(started.elapsed())
            .filter(|deadline| !deadline.is_zero())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "simulator request deadline elapsed",
                )
            })?;
        match read_request(&mut stream, cancelled, remaining_read_deadline) {
            Ok(request) if request.is_empty() && !cancelled.load(Ordering::Acquire) => continue,
            Ok(request) => break (stream, request),
            Err(_) if cancelled.load(Ordering::Acquire) => break (stream, Vec::new()),
            Err(error)
                if error.kind() == io::ErrorKind::TimedOut
                    && !cancelled.load(Ordering::Acquire) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    };
    let (method, path) = request_line(&request);
    let request_body = request_body(&request);
    let mut observed = ObservedRequest {
        method,
        path,
        bytes_received: request.len(),
        request_body_bytes: request_body.len(),
        request_body_sha256: hex::encode(Sha256::digest(request_body)),
        request_processed: false,
        cancelled: cancelled.load(Ordering::Acquire),
    };
    if observed.cancelled {
        return Ok(observed);
    }

    let body = plan.response_bytes();
    let headers = response_headers(&plan, body.len());
    match plan.delivery {
        Delivery::Immediate => {
            observed.request_processed = true;
            stream.write_all(headers.as_bytes())?;
            stream.flush()?;
            observed.cancelled =
                write_framed_body(&mut stream, &body, plan.framing, None, cancelled)?;
            finish_complete_response(&mut stream, observed.cancelled)?;
        }
        Delivery::SlowHeaders(delay) => {
            if sleep_cancellable(delay, cancelled) {
                observed.cancelled = true;
                return Ok(observed);
            }
            observed.request_processed = true;
            stream.write_all(headers.as_bytes())?;
            stream.flush()?;
            observed.cancelled =
                write_framed_body(&mut stream, &body, plan.framing, None, cancelled)?;
            finish_complete_response(&mut stream, observed.cancelled)?;
        }
        Delivery::SlowBody { chunk_bytes, delay } => {
            if chunk_bytes == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "slow-body chunk size must be positive",
                ));
            }
            observed.request_processed = true;
            stream.write_all(headers.as_bytes())?;
            stream.flush()?;
            observed.cancelled = write_framed_body(
                &mut stream,
                &body,
                plan.framing,
                Some((chunk_bytes, delay)),
                cancelled,
            )?;
            finish_complete_response(&mut stream, observed.cancelled)?;
        }
        Delivery::ResetBeforeBody => {
            stream.write_all(headers.as_bytes())?;
            stream.flush()?;
            // A declared body length with no body exercises truncated HTTP delivery.
        }
        Delivery::ResetAfterRequestProcessed { delay } => {
            observed.request_processed = true;
            if sleep_cancellable(delay, cancelled) {
                observed.cancelled = true;
            }
            // No response is emitted: a write client must treat this as ambiguous.
        }
    }
    Ok(observed)
}

fn finish_complete_response(stream: &mut TcpStream, cancelled: bool) -> io::Result<()> {
    if !cancelled {
        stream.flush()?;
        stream.shutdown(Shutdown::Write)?;
        // Give Windows' loopback stack time to deliver the FIN and buffered body before the
        // server thread drops the socket. Immediate drop is observably flaky under parallel CI.
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn read_request(
    stream: &mut TcpStream,
    cancelled: &AtomicBool,
    deadline: Duration,
) -> io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let started = Instant::now();
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if request.len().saturating_add(read) > MAX_REQUEST_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "synthetic request exceeded simulator limit",
                    ));
                }
                request.extend_from_slice(&buffer[..read]);
                if started.elapsed() >= deadline {
                    return Err(incomplete_request_timeout(&request));
                }
                if request_complete(&request) {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if cancelled.load(Ordering::Acquire) {
                    return Ok(request);
                }
                if started.elapsed() < deadline {
                    continue;
                }
                return Err(incomplete_request_timeout(&request));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(request)
}

fn incomplete_request_timeout(request: &[u8]) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "synthetic request body was incomplete (received {}, expected {:?})",
            request.len(),
            expected_request_bytes(request)
        ),
    )
}

fn request_complete(request: &[u8]) -> bool {
    expected_request_bytes(request).is_some_and(|expected| request.len() >= expected)
}

fn expected_request_bytes(request: &[u8]) -> Option<usize> {
    let header_end = find_bytes(request, b"\r\n\r\n")?;
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    Some(header_end + 4 + content_length)
}

fn request_line(request: &[u8]) -> (String, String) {
    let text = String::from_utf8_lossy(request);
    let mut parts = text.lines().next().unwrap_or_default().split_whitespace();
    (
        parts.next().unwrap_or_default().to_owned(),
        parts.next().unwrap_or_default().to_owned(),
    )
}

fn request_body(request: &[u8]) -> &[u8] {
    find_bytes(request, b"\r\n\r\n")
        .and_then(|header_end| request.get(header_end + 4..))
        .unwrap_or_default()
}

fn response_headers(plan: &ScenarioPlan, body_len: usize) -> String {
    let reason = match plan.http_status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Synthetic Status",
    };
    let charset = match plan.encoding {
        WireEncoding::Utf16Le | WireEncoding::Utf16Be => "; charset=utf-16",
        WireEncoding::Utf8 | WireEncoding::Utf8Bom
            if plan.fixture.content_type() == "application/json" =>
        {
            "; charset=utf-8"
        }
        WireEncoding::Utf8 | WireEncoding::Utf8Bom => "",
    };
    let framing = match plan.framing {
        ResponseFraming::ContentLength => format!("Content-Length: {body_len}\r\n"),
        ResponseFraming::ConnectionClose => String::new(),
        ResponseFraming::Chunked { .. } => "Transfer-Encoding: chunked\r\n".to_owned(),
        ResponseFraming::DeclaredContentLength { bytes } => {
            format!("Content-Length: {bytes}\r\n")
        }
    };
    let content_encoding = match plan.content_encoding {
        ResponseContentEncoding::None => "",
        ResponseContentEncoding::Identity => "Content-Encoding: identity\r\n",
        ResponseContentEncoding::Gzip => "Content-Encoding: gzip\r\n",
        ResponseContentEncoding::DuplicateIdentityThenGzip => {
            "Content-Encoding: identity\r\nContent-Encoding: gzip\r\n"
        }
    };
    let redirect_location = plan
        .redirect_location
        .as_deref()
        .filter(|value| !value.chars().any(char::is_control))
        .map(|value| format!("Location: {value}\r\n"))
        .unwrap_or_default();
    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}{}\r\n{}{}{}Connection: close\r\nX-Bridge-Synthetic: 1\r\n\r\n",
        plan.http_status,
        reason,
        plan.fixture.content_type(),
        charset,
        framing,
        content_encoding,
        redirect_location,
    )
}

fn write_framed_body(
    stream: &mut TcpStream,
    body: &[u8],
    framing: ResponseFraming,
    slow_delivery: Option<(usize, Duration)>,
    cancelled: &AtomicBool,
) -> io::Result<bool> {
    let framing_chunk = match framing {
        ResponseFraming::Chunked { chunk_bytes: 0 } => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "chunked framing size must be positive",
            ));
        }
        ResponseFraming::Chunked { chunk_bytes } => Some(chunk_bytes),
        _ => None,
    };
    let delivery_chunk = slow_delivery.map(|(chunk_bytes, _)| chunk_bytes);
    let chunk_bytes = match (framing_chunk, delivery_chunk) {
        (Some(left), Some(right)) => left.min(right),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => body.len().max(1),
    };

    for chunk in body.chunks(chunk_bytes) {
        if cancelled.load(Ordering::Acquire) {
            return Ok(true);
        }
        if matches!(framing, ResponseFraming::Chunked { .. }) {
            write!(stream, "{:X}\r\n", chunk.len())?;
            stream.write_all(chunk)?;
            stream.write_all(b"\r\n")?;
        } else {
            stream.write_all(chunk)?;
        }
        if let Some((_, delay)) = slow_delivery {
            if sleep_cancellable(delay, cancelled) {
                return Ok(true);
            }
        }
    }
    if matches!(framing, ResponseFraming::Chunked { .. }) {
        stream.write_all(b"0\r\n\r\n")?;
    }
    Ok(false)
}

fn sleep_cancellable(duration: Duration, cancelled: &AtomicBool) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(5)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_request_enforces_deadline_while_a_peer_drip_feeds_bytes() {
        let listener = bind_loopback_listener().expect("bind loopback listener");
        let address = listener.local_addr().expect("read listener address");
        let writer = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).expect("connect loopback listener");
            stream.set_nodelay(true).expect("disable Nagle buffering");
            for _ in 0..10 {
                if stream.write_all(b"x").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        let (mut stream, _) = listener.accept().expect("accept loopback peer");
        stream
            .set_read_timeout(Some(Duration::from_millis(25)))
            .expect("set read poll timeout");
        let cancelled = AtomicBool::new(false);
        let started = Instant::now();

        let error = read_request(&mut stream, &cancelled, Duration::from_millis(70))
            .expect_err("incomplete drip feed must not outlive its deadline");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "deadline enforcement must stay bounded"
        );
        writer.join().expect("drip-feed writer does not panic");
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
