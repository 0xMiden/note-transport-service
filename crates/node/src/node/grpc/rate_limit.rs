use std::net::{IpAddr, Ipv6Addr};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use governor::clock::{DefaultClock, QuantaInstant};
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};
use http::{HeaderValue, Request, Response, StatusCode};
use tonic::body::Body as TonicBody;
use tower::{Layer, Service};

/// Per-IP rate limiter. IPv6 addresses are keyed by their `/64` prefix so a single
/// routable prefix cannot bypass the limiter by rotating addresses.
pub type IpRateLimiter =
    RateLimiter<IpAddr, DashMapStateStore<IpAddr>, DefaultClock, NoOpMiddleware<QuantaInstant>>;

/// How often idle-IP entries are purged from the state store.
const RETAIN_INTERVAL: Duration = Duration::from_secs(60);

/// Configuration for rate limiting.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Sustained requests per second per IP.
    pub requests_per_second: NonZeroU32,
    /// Maximum burst size (tokens in the bucket).
    pub burst_size: NonZeroU32,
    /// Trust `X-Forwarded-For` / `X-Real-IP` when identifying the client.
    ///
    /// Only enable when the server sits behind a trusted reverse proxy that
    /// overwrites these headers — otherwise any client can bypass the limiter
    /// by setting the header themselves.
    pub trust_forwarded_headers: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: NonZeroU32::new(50).expect("50 is non-zero"),
            burst_size: NonZeroU32::new(100).expect("100 is non-zero"),
            trust_forwarded_headers: false,
        }
    }
}

/// Tower layer for per-IP rate limiting.
#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: Arc<IpRateLimiter>,
    trust_forwarded_headers: bool,
}

impl RateLimitLayer {
    /// Build a layer and spawn a background task that evicts idle-IP entries
    /// from the state store so it cannot grow unbounded. Must be called from
    /// inside a tokio runtime.
    pub fn new(config: &RateLimitConfig) -> Self {
        let quota = Quota::per_second(config.requests_per_second).allow_burst(config.burst_size);
        let limiter = Arc::new(RateLimiter::dashmap(quota));

        tokio::spawn({
            let limiter = limiter.clone();
            async move {
                let mut interval = tokio::time::interval(RETAIN_INTERVAL);
                interval.tick().await; // skip the immediate first tick
                loop {
                    interval.tick().await;
                    limiter.retain_recent();
                }
            }
        });

        Self {
            limiter,
            trust_forwarded_headers: config.trust_forwarded_headers,
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: self.limiter.clone(),
            trust_forwarded_headers: self.trust_forwarded_headers,
        }
    }
}

/// Rate-limiting service wrapping an inner service.
#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: Arc<IpRateLimiter>,
    trust_forwarded_headers: bool,
}

impl<S> RateLimitService<S> {
    /// Client IP used for rate-limit bucketing, or `None` if it cannot be
    /// determined (in which case the request is allowed through — we prefer
    /// that over lumping unrelated requests into a shared bucket).
    fn extract_ip<B>(&self, req: &Request<B>) -> Option<IpAddr> {
        if self.trust_forwarded_headers {
            if let Some(ip) = req.headers().get("x-forwarded-for").and_then(parse_forwarded_for) {
                return Some(ip);
            }
            if let Some(ip) = req.headers().get("x-real-ip").and_then(parse_single_ip) {
                return Some(ip);
            }
        }

        req.extensions()
            .get::<tonic::transport::server::TcpConnectInfo>()
            .and_then(tonic::transport::server::TcpConnectInfo::remote_addr)
            .map(|addr| addr.ip())
    }
}

fn parse_forwarded_for(value: &HeaderValue) -> Option<IpAddr> {
    value.to_str().ok()?.split(',').next()?.trim().parse().ok()
}

fn parse_single_ip(value: &HeaderValue) -> Option<IpAddr> {
    value.to_str().ok()?.trim().parse().ok()
}

/// A single attacker with a routable `/64` can otherwise rotate through 2^64
/// addresses and dodge the limiter entirely.
fn bucket(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => {
            let mut octets = v6.octets();
            for o in &mut octets[8..] {
                *o = 0;
            }
            IpAddr::V6(Ipv6Addr::from(octets))
        },
    }
}

/// Trailers-only gRPC response: HTTP 200 with `grpc-status: 8`
/// (`RESOURCE_EXHAUSTED`) — the canonical gRPC mapping for rate-limit rejection.
fn rate_limit_exceeded_response() -> Response<TonicBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/grpc")
        .header("grpc-status", "8")
        .header("grpc-message", "rate limit exceeded")
        .body(TonicBody::empty())
        .expect("static rate-limit response is valid")
}

impl<S, ReqBody> Service<Request<ReqBody>> for RateLimitService<S>
where
    S: Service<Request<ReqBody>, Response = Response<TonicBody>> + Clone + Send + 'static,
    S::Future: Send,
    ReqBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future =
        futures::future::Either<S::Future, std::future::Ready<Result<Self::Response, Self::Error>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        if let Some(ip) = self.extract_ip(&req) {
            if self.limiter.check_key(&bucket(ip)).is_err() {
                return futures::future::Either::Right(std::future::ready(Ok(
                    rate_limit_exceeded_response(),
                )));
            }
        }

        futures::future::Either::Left(self.inner.call(req))
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn req_with_headers(headers: &[(&'static str, &str)]) -> Request<()> {
        let mut req = Request::builder().body(()).unwrap();
        for (name, value) in headers {
            req.headers_mut().insert(*name, HeaderValue::from_str(value).unwrap());
        }
        req
    }

    fn service(trust_forwarded_headers: bool) -> RateLimitService<()> {
        let cfg = RateLimitConfig {
            trust_forwarded_headers,
            ..Default::default()
        };
        RateLimitLayer::new(&cfg).layer(())
    }

    #[tokio::test]
    async fn extract_ip_honors_forwarded_for_when_trusted() {
        let svc = service(true);
        let req = req_with_headers(&[("x-forwarded-for", "203.0.113.1, 10.0.0.1")]);
        assert_eq!(svc.extract_ip(&req), Some("203.0.113.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn extract_ip_ignores_forwarded_for_when_untrusted() {
        let svc = service(false);
        let req = req_with_headers(&[("x-forwarded-for", "203.0.113.1")]);
        assert_eq!(svc.extract_ip(&req), None);
    }

    #[tokio::test]
    async fn extract_ip_falls_back_to_real_ip_when_trusted() {
        let svc = service(true);
        let req = req_with_headers(&[("x-real-ip", "198.51.100.7")]);
        assert_eq!(svc.extract_ip(&req), Some("198.51.100.7".parse().unwrap()));
    }

    #[tokio::test]
    async fn extract_ip_ignores_malformed_forwarded_for() {
        let svc = service(true);
        let req = req_with_headers(&[("x-forwarded-for", "not-an-ip")]);
        assert_eq!(svc.extract_ip(&req), None);
    }

    #[test]
    fn bucket_masks_ipv6_to_slash_64() {
        let ip: IpAddr = "2001:db8:1:2:dead:beef:cafe:1".parse().unwrap();
        let expected: IpAddr = "2001:db8:1:2::".parse().unwrap();
        assert_eq!(bucket(ip), expected);
    }

    #[test]
    fn bucket_preserves_ipv4() {
        let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));
        assert_eq!(bucket(ip), ip);
    }

    #[tokio::test]
    async fn over_quota_requests_are_rejected() {
        let cfg = RateLimitConfig {
            requests_per_second: NonZeroU32::new(1).unwrap(),
            burst_size: NonZeroU32::new(1).unwrap(),
            trust_forwarded_headers: true,
        };
        let layer = RateLimitLayer::new(&cfg);
        let ip: IpAddr = "203.0.113.10".parse().unwrap();

        assert!(layer.limiter.check_key(&ip).is_ok(), "first request should pass");
        assert!(layer.limiter.check_key(&ip).is_err(), "second request should be rate-limited");
    }
}
