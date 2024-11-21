use crate::*;
use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{request::Parts, StatusCode},
    middleware::Next,
    response::Response,
};
use std::{fmt::Display, net::SocketAddr, str::FromStr};

/// Stores either a full IPv4 address or a /64 IPv6 subnet.
///
/// Used for rate limiting and identifying uploading clients.
#[derive(Debug, PartialEq, Eq, Hash)]
pub enum IpPrefix {
    V4([u8; 4]),
    V6([u8; 8]),
}

impl From<SocketAddr> for IpPrefix {
    /// Convert a SocketAddr's IPv4 or IPv6 address to an IpPrefix. Infallible.
    fn from(addr: SocketAddr) -> Self {
        match addr.ip() {
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
    /// This makes storing and comparing values in a database easier.
    /// Moreover, it enabled parsing the canonical representation
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

    /// Parse the canonical string represantation back into the struct.
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

/// This extractor conveniently allows us to extract a client's IP as an IpPrefix.
///
/// It's likely that in the future the source of the IpPrefix will not be the SocketAddr
/// at all but instead the X-Forwarded-For header received by the reverse proxy.
/// This extractor marks a convenient and centralized place where the source of the extracted
/// IpPrefix can be adjusted. Parts contains the HTTP headers, so switching should be easy.
#[derive(Debug, PartialEq, Eq)]
pub struct ExtractIpPrefix(pub IpPrefix);

#[axum::async_trait]
impl<S> FromRequestParts<S> for ExtractIpPrefix
where
    S: Send + Sync,
{
    type Rejection = ();

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .expect("cannot extract client's IP")
            .0;
        Ok(Self(IpPrefix::from(ip)))
    }
}

/// Custom rate-limiting solution built on top of IpPrefix.
pub async fn ip_prefix_ratelimiter(
    State(aps): State<AppState>,
    ExtractIpPrefix(eip): ExtractIpPrefix,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Count the request.
    // aps.rate_limiter.
    Ok(next.run(request).await)
}
