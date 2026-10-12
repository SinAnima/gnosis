//! Block destinations that are not public HTTP. Checked on the start URL and
//! on every redirect the Gnosis crawler follows. Spider may open its own
//! connection; when it does, Gnosis still rejects the URL before the page is
//! stored, which is not a DNS-rebinding guarantee at the socket.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use url::Url;

pub fn address_is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_blocked(v4),
        IpAddr::V6(v6) => ipv6_blocked(v6),
    }
}

fn ipv4_blocked(v4: Ipv4Addr) -> bool {
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
        || v4.octets() == [169, 254, 169, 254]
}

fn ipv6_blocked(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
        return true;
    }
    if let Some(v4) = v6.to_ipv4_mapped() {
        return ipv4_blocked(v4);
    }
    let octets = v6.octets();
    let unique_local = (octets[0] & 0xfe) == 0xfc;
    let link_local = octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80;
    unique_local || link_local
}

fn private_block_reason(ip: IpAddr) -> String {
    format!(
        "blocked address {ip}; pass --allow-private-network to scan loopback, private, or link-local hosts"
    )
}

fn is_metadata_ip(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets() == [169, 254, 169, 254])
}

pub fn host_is_metadata(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "metadata.google.internal" || host == "metadata.google.com"
}

/// `Ok(())` when the URL may be fetched. `Err` is an acquisition reason.
pub fn check_url(raw: &str, allow_private: bool) -> Result<(), String> {
    let url = Url::parse(raw).map_err(|e| format!("invalid URL: {e}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!("unsupported URL scheme '{}'", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with credentials are not supported".into());
    }
    let Some(host) = url.host_str() else {
        return Err("URL has no host".into());
    };
    if host_is_metadata(host) {
        return Err("cloud metadata hosts are blocked".into());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_metadata_ip(ip) {
            return Err(format!("blocked address {ip}"));
        }
        if !allow_private && address_is_blocked(ip) {
            return Err(private_block_reason(ip));
        }
        return Ok(());
    }
    if allow_private {
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let mut addrs = match (host, port).to_socket_addrs() {
        Ok(iter) => iter,
        Err(e) => return Err(format!("could not resolve {host}: {e}")),
    };
    let mut saw = false;
    for addr in addrs.by_ref() {
        saw = true;
        if is_metadata_ip(addr.ip()) {
            return Err(format!("blocked address {}", addr.ip()));
        }
        if address_is_blocked(addr.ip()) {
            return Err(private_block_reason(addr.ip()));
        }
    }
    if !saw {
        return Err(format!("could not resolve {host}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_and_metadata_literals() {
        assert!(address_is_blocked("127.0.0.1".parse().unwrap()));
        assert!(address_is_blocked("10.1.2.3".parse().unwrap()));
        assert!(address_is_blocked("192.168.0.1".parse().unwrap()));
        assert!(address_is_blocked("169.254.169.254".parse().unwrap()));
        assert!(address_is_blocked("::1".parse().unwrap()));
        assert!(check_url("http://169.254.169.254/", false).is_err());
        assert!(check_url("http://metadata.google.internal/", false).is_err());
        assert!(check_url("http://127.0.0.1/", true).is_ok());
        assert!(check_url("http://user:pw@example.com/", false).is_err());
    }
}
