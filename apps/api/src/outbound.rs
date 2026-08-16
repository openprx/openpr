use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Truncates diagnostics by Unicode scalar count so persistence limits never split UTF-8.
pub fn truncate_string(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value
    } else {
        value.chars().take(max_chars).collect()
    }
}

fn outbound_allowlist() -> String {
    crate::config::runtime().outbound.allowlist_csv()
}

fn outbound_private_targets_allowed() -> bool {
    crate::config::runtime().outbound.allow_private
}

/// Matches a host, optionally including its port, against a comma-separated allowlist.
pub fn host_is_allowlisted(host: &str, port: Option<u16>, allowlist: &str) -> bool {
    let host = normalize_host(host);
    allowlist
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let entry = entry.to_ascii_lowercase();
            match entry.rsplit_once(':') {
                Some((entry_host, entry_port))
                    if !entry_port.is_empty() && entry_port.chars().all(|character| character.is_ascii_digit()) =>
                {
                    normalize_host(entry_host) == host && port.is_some_and(|value| value.to_string() == entry_port)
                }
                _ => normalize_host(&entry) == host,
            }
        })
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

/// Rejects addresses that must never be reachable from a user-supplied endpoint.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => is_blocked_ipv4(ipv4),
        IpAddr::V6(ipv6) => is_blocked_ipv6(ipv6),
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 0
        || ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || (first == 100 && (64..128).contains(&second))
        || (first == 192 && second == 0)
        || (first == 198 && (18..20).contains(&second))
        || first >= 240
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return true;
    }
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(mapped);
    }
    let segments = ip.segments();
    let [first, second, ..] = segments;
    if first == 0x0064 && second == 0xff9b {
        return if segments[2..6].iter().all(|segment| *segment == 0) {
            is_blocked_ipv4(embedded_ipv4(segments[6], segments[7]))
        } else {
            true
        };
    }
    if first == 0x2002 {
        return is_blocked_ipv4(embedded_ipv4(segments[1], segments[2]));
    }
    if segments[..6].iter().all(|segment| *segment == 0) {
        return is_blocked_ipv4(embedded_ipv4(segments[6], segments[7]));
    }
    (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xffc0) == 0xfec0
        || first == 0x2001 && second == 0x0db8
}

fn embedded_ipv4(high: u16, low: u16) -> Ipv4Addr {
    Ipv4Addr::from((u32::from(high) << 16) | u32::from(low))
}

fn literal_host_ip(host: &str) -> Option<IpAddr> {
    normalize_host(host).parse::<IpAddr>().ok()
}

/// Performs scheme, credential and literal-address checks without DNS resolution.
pub fn parse_outbound_url(raw: &str) -> Result<reqwest::Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("endpoint must not be empty".to_string());
    }
    let url = reqwest::Url::parse(trimmed).map_err(|error| format!("endpoint is not a valid absolute URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "endpoint scheme {} is not allowed, use http or https",
            url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("endpoint must not embed credentials".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "endpoint must contain a host".to_string())?
        .to_string();
    if outbound_private_targets_allowed()
        || host_is_allowlisted(&host, url.port_or_known_default(), &outbound_allowlist())
    {
        return Ok(url);
    }
    if literal_host_ip(&host).is_some_and(is_blocked_ip) {
        return Err(format!("endpoint host {host} points at a blocked address"));
    }
    Ok(url)
}

/// Performs full outbound target validation, including DNS resolution.
pub async fn validate_outbound_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = parse_outbound_url(raw)?;
    if outbound_private_targets_allowed() {
        return Ok(url);
    }
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default();
    if host_is_allowlisted(&host, port, &outbound_allowlist()) || literal_host_ip(&host).is_some() {
        return Ok(url);
    }
    let addresses = tokio::net::lookup_host((host.as_str(), port.unwrap_or(443)))
        .await
        .map_err(|error| format!("endpoint host {host} could not be resolved: {error}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(format!("endpoint host {host} could not be resolved"));
    }
    if addresses.iter().any(|address| is_blocked_ip(address.ip())) {
        return Err(format!("endpoint host {host} resolves to a blocked address"));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::{host_is_allowlisted, is_blocked_ip, truncate_string};

    #[test]
    fn diagnostics_truncate_on_character_boundaries() {
        assert_eq!(truncate_string("abcdef".to_string(), 3), "abc");
        assert_eq!(truncate_string("好好好".to_string(), 2), "好好");
    }

    #[test]
    fn private_and_transition_addresses_are_blocked() {
        for raw in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            let address = raw.parse().ok();
            assert!(address.is_some_and(is_blocked_ip), "{raw} must be blocked");
        }
    }

    #[test]
    fn host_allowlist_matches_exact_host_and_optional_port() {
        assert!(host_is_allowlisted("API", Some(8080), " api , webhook "));
        assert!(host_is_allowlisted("api", Some(8080), "api:8080"));
        assert!(!host_is_allowlisted("api", Some(9090), "api:8080"));
        assert!(!host_is_allowlisted("evil.example.com", Some(443), "api,webhook"));
    }
}
