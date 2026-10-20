//! Post-DNS SSRF guard shared by every host-side fetch.
//!
//! Host-name allowlists are not enough: a permitted name can resolve to loopback or a
//! LAN address, and an unfiltered fetch target (`cache_images`) can be pointed at one
//! outright. Every fetch target is therefore checked *after* DNS resolution and refused
//! when any resolved address is not publicly routable.

use std::net::{IpAddr, Ipv6Addr};

/// Cap on a single cached-image download, in bytes. Downloads stream, so this is
/// enforced chunk-by-chunk while reading — a lying or missing `Content-Length`
/// cannot blow past it.
pub const MAX_IMAGE_DOWNLOAD_BYTES: u64 = 10 * 1024 * 1024;

/// True when `ip` is never a legitimate public fetch target: loopback, private
/// (RFC 1918 / IPv6 ULA), link-local, carrier-grade NAT, unspecified, or multicast.
pub fn ip_is_non_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                // 100.64.0.0/10 — carrier-grade NAT, not publicly routable.
                || (u32::from(*v4) & 0xFFC0_0000) == 0x6440_0000
                // 198.18.0.0/15 — benchmarking (RFC 2544), not publicly routable.
                || (u32::from(*v4) & 0xFFFE_0000) == 0xC612_0000
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unicast_link_local()
                || is_unique_local(v6)
        }
    }
}

/// fc00::/7 — IPv6 unique-local addresses. `Ipv6Addr` has no stable helper for this.
fn is_unique_local(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}

/// Resolve `host` and refuse when it fails to resolve or when *any* resolved address
/// is non-public. Refusing on the first match alone is not enough: a resolver can
/// return a mix of public and private addresses, and the HTTP client may try each in
/// turn. Fails closed — an unresolvable host is refused, since it could not be
/// fetched anyway.
pub async fn ensure_host_is_public(host: &str) -> Result<(), String> {
    // `Url::host_str` keeps the brackets on IPv6 literals (`[::1]`); the resolver
    // wants the bare address.
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|e| format!("Fetch to '{host}' refused: could not resolve host: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!(
            "Fetch to '{host}' refused: host resolved to no addresses"
        ));
    }
    if let Some(bad) = addrs.iter().map(|a| a.ip()).find(ip_is_non_public) {
        return Err(format!(
            "Fetch to '{host}' refused: host resolves to non-public address {bad}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn loopback_is_blocked() {
        assert!(ip_is_non_public(&v4(127, 0, 0, 1)));
        assert!(ip_is_non_public(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn private_ranges_are_blocked() {
        for ip in [
            v4(10, 0, 0, 1),
            v4(172, 16, 0, 1),
            v4(172, 31, 255, 254),
            v4(192, 168, 1, 1),
        ] {
            assert!(ip_is_non_public(&ip), "{ip}");
        }
        // Just outside the private ranges stays public.
        assert!(!ip_is_non_public(&v4(172, 15, 255, 255)));
        assert!(!ip_is_non_public(&v4(172, 32, 0, 1)));
    }

    #[test]
    fn link_local_and_cgnat_are_blocked() {
        assert!(ip_is_non_public(&v4(169, 254, 10, 20)));
        assert!(ip_is_non_public(&IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(ip_is_non_public(&v4(100, 64, 0, 1)));
        assert!(ip_is_non_public(&v4(100, 127, 255, 255)));
        assert!(!ip_is_non_public(&v4(100, 63, 255, 255)));
        assert!(!ip_is_non_public(&v4(100, 128, 0, 1)));
    }

    #[test]
    fn unspecified_and_multicast_are_blocked() {
        assert!(ip_is_non_public(&v4(0, 0, 0, 0)));
        assert!(ip_is_non_public(&v4(224, 0, 0, 1)));
        assert!(ip_is_non_public(&IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn ipv6_unique_local_is_blocked() {
        assert!(ip_is_non_public(&IpAddr::V6(Ipv6Addr::new(
            0xfd00, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(!ip_is_non_public(&IpAddr::V6(Ipv6Addr::new(
            0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111
        ))));
    }

    #[test]
    fn public_ips_are_allowed() {
        assert!(!ip_is_non_public(&v4(8, 8, 8, 8)));
        assert!(!ip_is_non_public(&v4(1, 1, 1, 1)));
        assert!(!ip_is_non_public(&v4(203, 0, 113, 7)));
    }

    #[tokio::test]
    async fn loopback_literal_is_refused() {
        let err = ensure_host_is_public("127.0.0.1").await.unwrap_err();
        assert!(err.contains("non-public"), "{err}");
    }

    #[tokio::test]
    async fn ipv6_loopback_literal_is_refused() {
        let err = ensure_host_is_public("::1").await.unwrap_err();
        assert!(err.contains("non-public"), "{err}");
    }

    #[tokio::test]
    async fn bracketed_ipv6_loopback_is_refused() {
        // `Url::host_str` hands us `[::1]` with brackets.
        let err = ensure_host_is_public("[::1]").await.unwrap_err();
        assert!(err.contains("non-public"), "{err}");
    }

    #[tokio::test]
    async fn localhost_resolves_to_loopback_and_is_refused() {
        // `localhost` resolves via /etc/hosts — no real network needed.
        let err = ensure_host_is_public("localhost").await.unwrap_err();
        assert!(err.contains("non-public"), "{err}");
    }

    #[test]
    fn benchmarking_range_is_blocked() {
        assert!(ip_is_non_public(&v4(198, 18, 0, 1)));
        assert!(ip_is_non_public(&v4(198, 19, 255, 255)));
        assert!(!ip_is_non_public(&v4(198, 17, 255, 255)));
        assert!(!ip_is_non_public(&v4(198, 20, 0, 1)));
    }

    #[tokio::test]
    async fn unresolvable_host_is_refused() {
        // `.invalid` never resolves on real DNS; in sandboxes with a wildcard
        // resolver it lands in the non-public benchmarking range — refused either way.
        let err = ensure_host_is_public("nonexistent.invalid").await;
        assert!(err.is_err());
    }
}
