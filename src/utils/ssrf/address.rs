use std::net::{IpAddr, Ipv4Addr};

/// Mirror the pinned SDK's `CPython` 3.13 IP classification, including mapped IPv4.
///
/// This is a compatibility classification, not an automatically updated routing
/// registry. In particular, Python considers deprecated IPv6 site-local addresses
/// (`fec0::/10`) global. Apply additional application policy if those are routed
/// internally. Scoped IPv6 strings are classified by their underlying address.
#[must_use]
pub fn is_safe_address(address: &str) -> bool {
    let unscoped = match address.split_once('%') {
        Some((ip, scope)) if ip.contains(':') && !scope.is_empty() && !scope.contains('%') => ip,
        Some(_) => return false,
        None => address,
    };
    unscoped.parse().is_ok_and(is_safe_ip)
}

pub(super) fn is_safe_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => safe_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return safe_v4(mapped);
            }
            let n = u128::from(ip);
            // CPython's reserved blocks leave 2000::/3 and fec0::/10. Multicast,
            // unique-local, and link-local are excluded separately upstream.
            if !(n >> 125 == 1 || n >> 118 == 0x3fb) {
                return false;
            }
            let private = [
                (0x2001_u128 << 112, 23),
                (0x2001_0db8 << 96, 32),
                (0x2002 << 112, 16),
                (0x3fff << 112, 20),
            ];
            let exceptions = [
                (0x2001_0001_0000_0000_0000_0000_0000_0001, 128),
                (0x2001_0001_0000_0000_0000_0000_0000_0002, 128),
                (0x2001_0003_0000_0000_0000_0000_0000_0000, 32),
                (0x2001_0004_0112_0000_0000_0000_0000_0000, 48),
                (0x2001_0020_0000_0000_0000_0000_0000_0000, 28),
                (0x2001_0030_0000_0000_0000_0000_0000_0000, 28),
            ];
            !private.iter().any(|&(base, bits)| prefix6(n, base, bits))
                || exceptions
                    .iter()
                    .any(|&(base, bits)| prefix6(n, base, bits))
        }
    }
}

fn prefix6(ip: u128, base: u128, bits: u32) -> bool {
    ip >> (128 - bits) == base >> (128 - bits)
}

fn safe_v4(ip: Ipv4Addr) -> bool {
    let n = u32::from(ip);
    if n == 0xc000_0009 || n == 0xc000_000a {
        return true;
    }
    // Private, special-use, CGNAT, multicast, and reserved ranges from the
    // pinned Python interpreter. Exceptions above are 192.0.0.9 and .10.
    ![
        (0x0000_0000, 8),
        (0x0a00_0000, 8),
        (0x6440_0000, 10),
        (0x7f00_0000, 8),
        (0xa9fe_0000, 16),
        (0xac10_0000, 12),
        (0xc000_0000, 24),
        (0xc000_0200, 24),
        (0xc0a8_0000, 16),
        (0xc612_0000, 15),
        (0xc633_6400, 24),
        (0xcb00_7100, 24),
        (0xe000_0000, 4),
        (0xf000_0000, 4),
    ]
    .iter()
    .any(|&(base, bits)| n >> (32 - bits) == base >> (32 - bits))
}
