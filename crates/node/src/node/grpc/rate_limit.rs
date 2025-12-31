use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::task::{Context, Poll};

use governor::clock::{DefaultClock, QuantaInstant};
use governor::middleware::NoOpMiddleware;
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};
use http::{Request, Response, StatusCode};
use tonic::body::Body as TonicBody;
use tower::{Layer, Service};

/// Rate limiter type
pub type IpRateLimiter =
    RateLimiter<IpAddr, DashMapStateStore<IpAddr>, DefaultClock, NoOpMiddleware<QuantaInstant>>;

/// Configuration for rate limiting
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum requests per second per IP
    pub requests_per_second: u32,
    /// Allow temporary spikes
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self { requests_per_second: 50, burst_size: 100 }
    }
}

/// Tower layer for per-IP rate limiting
#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: Arc<IpRateLimiter>,
}

impl RateLimitLayer {
    /// Create a new rate limit layer from config
    pub fn new(config: &RateLimitConfig) -> Self {
        let quota = Quota::per_second(
            NonZeroU32::new(config.requests_per_second).unwrap_or(NonZeroU32::MIN),
        )
        .allow_burst(NonZeroU32::new(config.burst_size).unwrap_or(NonZeroU32::MIN));

        Self {
            limiter: Arc::new(RateLimiter::dashmap(quota)),
        }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService { inner, limiter: self.limiter.clone() }
    }
}

/// Rate limiting service that wraps an inner service
#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: Arc<IpRateLimiter>,
}

impl<S> RateLimitService<S> {
    /// Extract client IP from request
    ///
    /// Checks headers in order:
    /// 1. `x-forwarded-for` (first IP in the chain, from reverse proxy)
    /// 2. `x-real-ip` (common alternative header)
    /// 3. Connected peer address (direct connection)
    /// 4. Fallback to localhost
    fn extract_ip<B>(req: &Request<B>) -> IpAddr {
        // Try X-Forwarded-For header first (for reverse proxy setups)
        if let Some(forwarded_for) = req.headers().get("x-forwarded-for") {
            if let Ok(header_str) = forwarded_for.to_str() {
                // Take the first IP in the chain (original client)
                if let Some(first_ip) = header_str.split(',').next() {
                    if let Ok(ip) = first_ip.trim().parse::<IpAddr>() {
                        return ip;
                    }
                }
            }
        }

        // Try X-Real-IP header
        if let Some(real_ip) = req.headers().get("x-real-ip") {
            if let Ok(header_str) = real_ip.to_str() {
                if let Ok(ip) = header_str.trim().parse::<IpAddr>() {
                    return ip;
                }
            }
        }

        // Fall back to connected peer address from extensions
        if let Some(addr) = req.extensions().get::<tonic::transport::server::TcpConnectInfo>() {
            if let Some(remote_addr) = addr.remote_addr() {
                return remote_addr.ip();
            }
        }

        // Last resort: use localhost
        // Ensures we don't fail the request, but rate limits unknown sources together
        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    }
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
        let ip = Self::extract_ip(&req);

        // Check rate limit
        if self.limiter.check_key(&ip).is_err() {
            // return 429
            let response = Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("content-type", "application/grpc")
                .header("grpc-status", "14") // unavailable
                .header("grpc-message", "rate limit exceeded")
                .body(TonicBody::empty())
                .unwrap();

            return futures::future::Either::Right(std::future::ready(Ok(response)));
        }

        futures::future::Either::Left(self.inner.call(req))
    }
}
