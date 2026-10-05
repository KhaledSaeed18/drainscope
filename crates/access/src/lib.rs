//! Access control shared by the privileged services (sampler, probe): polkit authorization
//! and the side-channel mitigations (per-user rate limits, quantization).

pub mod auth;
pub mod limits;

pub use auth::{AuthFuture, Authorizer, Fixed, Polkit};
pub use limits::{RateLimiter, quantize};
