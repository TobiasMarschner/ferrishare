//! Identify and rate-limit users through their IPv4 address or /64 IPv6 subnet

use crate::*;
use axum::{
    extract::{ConnectInfo, FromRef, FromRequestParts, Request, State},
    http::{request::Parts, StatusCode},
    middleware::Next,
    response::Response,
};
use std::{
    fmt::Display,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

/// Stores either a full IPv4 address or a /64 IPv6 subnet
///
/// Used for rate limiting and identifying uploading clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpPrefix {
    V4([u8; 4]),
    V6([u8; 8]),
}

impl From<IpAddr> for IpPrefix {
    /// Convert a SocketAddr's IPv4 or IPv6 address to an IpPrefix. Infallible.
    fn from(addr: IpAddr) -> Self {
        match addr {
            std::net::IpAddr::V4(ipv4_addr) => IpPrefix::V4(ipv4_addr.octets()),
            std::net::IpAddr::V6(ipv6_addr) => {
                // I originally used "ipv6_addr.octets()[..8].try_into().unwrap()" for this,
                // but this might (?) be better since it removes the try_into and unwrap.
                let [b0, b1, b2, b3, b4, b5, b6, b7, ..] = ipv6_addr.octets();
                IpPrefix::V6([b0, b1, b2, b3, b4, b5, b6, b7])
            }
        }
    }
}

impl Display for IpPrefix {
    /// Use a custom string-serialization that is constant-size by using
    /// hexadeximal encoding. This also makes for canonical encodings.
    ///
    /// This makes storing and comparing values in the database easier.
    /// Moreover, it enables parsing the canonical representation
    /// back into an UploadIpPrefix.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpPrefix::V4([b0, b1, b2, b3]) => {
                write!(f, "v4_{b0:02x}{b1:02x}{b2:02x}{b3:02x}")
            }
            IpPrefix::V6([b0, b1, b2, b3, b4, b5, b6, b7]) => {
                write!(
                    f,
                    "v6_{b0:02x}{b1:02x}{b2:02x}{b3:02x}{b4:02x}{b5:02x}{b6:02x}{b7:02x}"
                )
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct UploadIpPrefixParseError;

impl FromStr for IpPrefix {
    type Err = UploadIpPrefixParseError;

    /// Parse the canonical string represantation back into an IpPrefix.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, octet_str) = s.split_at_checked(3).ok_or(UploadIpPrefixParseError)?;
        let octets = hex::decode(octet_str).map_err(|_| UploadIpPrefixParseError)?;

        if let ("v4_", [b0, b1, b2, b3]) = (prefix, octets.as_slice()) {
            Ok(IpPrefix::V4([*b0, *b1, *b2, *b3]))
        } else if let ("v6_", [b0, b1, b2, b3, b4, b5, b6, b7]) = (prefix, octets.as_slice()) {
            Ok(IpPrefix::V6([*b0, *b1, *b2, *b3, *b4, *b5, *b6, *b7]))
        } else {
            Err(UploadIpPrefixParseError)
        }
    }
}

impl IpPrefix {
    /// Prints the contained IP-Prefix in pretty / human-readable notation for logging.
    pub fn pretty_print(&self) -> String {
        match self {
            IpPrefix::V4([b0, b1, b2, b3]) => format!("{b0}.{b1}.{b2}.{b3}"),
            IpPrefix::V6([b0, b1, b2, b3, b4, b5, b6, b7]) => {
                format!("{b0:02x}{b1:02x}:{b2:02x}{b3:02x}:{b4:02x}{b5:02x}:{b6:02x}{b7:02x}::/64")
            }
        }
    }
}

/// This extractor conveniently allows us to extract a client's IP as an IpPrefix.
///
/// This extractor respects the global configuration's proxy_depth.
/// If set to 0 the SocketAddr will be used to construct the IpPrefix.
/// If set to 1 or higher the X-Forwarded-For header will be dissected to construct the IpPrefix.
#[derive(Debug, PartialEq, Eq)]
pub struct ExtractIpPrefix(pub IpPrefix);

#[axum::async_trait]
impl<S> FromRequestParts<S> for ExtractIpPrefix
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // There are two possible sources for the client's "real" IP address:
        // 1) The SocketAddr of the machine directly communicating with us.
        // 2) One of the IP addresses listed in the X-Forwarded-For header.
        //
        // Which one it is depends on the reverse-proxy settings.
        // If proxy_depth is 0 there is no reverse-proxy, and we work directly with
        // the SocketAddr. Otherwise, we extract the IP address from the X-Forwarded-For header,
        // taking care to select the right one in the chain.
        let ip: IpAddr = match AppState::from_ref(state).conf.proxy_depth {
            0 => {
                parts
                    .extensions
                    .get::<ConnectInfo<SocketAddr>>()
                    .map_or_else( || { AppError::err( StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to extract SocketAddr from request; your reverse-proxy configuration might be incorrect") },
                        |v| Ok(v.0.ip()),
                    )?
            }
            s => {
                let forwarded_ips = parts
                    .headers
                    .get("X-Forwarded-For")
                    .map_or("", |v| v.to_str().unwrap_or_default())
                    .split(' ')
                    .collect_vec();

                forwarded_ips
                    .get(forwarded_ips.len() - (s as usize))
                    .and_then(|v| IpAddr::from_str(v).ok())
                    .map_or_else( || { AppError::err(StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to extract IP from X-Forwarded-For header; your reverse-proxy configuration might be incorrect") },
                        Ok
                    )?
            }
        };

        Ok(Self(IpPrefix::from(ip)))
    }
}

/// Custom rate-limiting middleware that uses the IpPrefix extractor
pub async fn ip_prefix_ratelimiter(
    State(aps): State<AppState>,
    ExtractIpPrefix(eip): ExtractIpPrefix,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    // Acquire a writing reference to the rate-limiter.
    let mut rl = aps.rate_limiter.write().await;
    // Insert the key if it wasn't already there and update its counter.
    let counter = *rl
        .entry(eip)
        .and_modify(|v| *v += 1)
        .or_insert(1);
    // Drop our borrow, or we can only process one request at a time, lol.
    drop(rl);
    // Rate limit, if need be.
    if counter <= aps.conf.daily_request_limit_per_ip {
        Ok(next.run(request).await)
    } else {
        AppError::err(
            StatusCode::TOO_MANY_REQUESTS,
            "too many requests, come back later",
        )
    }
}
