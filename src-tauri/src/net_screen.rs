// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The public-internet screen: may PM dial this address for something it doesn't trust?
//!
//! Calendar feeds (a URL the user pasted from anywhere) use it today; a mod's `net.fetch` will use
//! it too (Mods spec §1.5). Both must reach the public internet and nothing else: not this computer
//! (Ollama, a dev server, the MCP socket), not the LAN or the router, not a cloud metadata service,
//! and not whatever a translator or tunnel would turn an address into.
//!
//! Two layers, so the decision stays pure:
//! - [`ip_is_blocked`] is the verdict on one address.
//! - [`resolve_and_screen`] is the resolver edge. It turns a host into addresses, screens every one
//!   and returns them for the caller to PIN onto its HTTP client (reqwest's `resolve_to_addrs`),
//!   so the address screened is the address dialled, with no DNS-rebinding window between the two.
//!   The pin holds only on a direct connection: a client that goes through a proxy hands the NAME
//!   to the proxy, which resolves it again.
//!
//! IPv4 is a block-list: the space is fully assigned, so what isn't special-purpose is public. IPv6
//! is an allow-list: only global unicast `2000::/3` can be public, less the special blocks inside
//! it, so a range assigned outside it later is refused without a code change. A form that carries
//! an IPv4 address the packet really travels to (mapped, compatible, NAT64, 6to4) is judged AS that
//! IPv4. The registries this follows are IANA's IPv4 and IPv6 special-purpose address registries.
//!
//! No screen can see a NAT64 translator on an operator's own prefix (it looks like any global
//! address) or a proxy's own lookup. TLS still has to verify the name, so either one reaches at
//! most a blind connect, never a readable response.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

/// Why a host was refused. Each caller words it in its own voice (a calendar feed and a mod say
/// different things), so the screen returns the reason, never a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The host is, or resolved to at least one, address outside the public internet.
    NotPublic,
    /// The host resolved to no address, or the lookup failed.
    Unresolved,
}

/// The host as an IP literal, if it is one. A URL's `host_str()` brackets an IPv6 literal
/// (`[::1]`); a bare one parses as well.
pub fn literal_ip(host: &str) -> Option<IpAddr> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse().ok()
}

/// True if PM must not dial `ip` for an untrusted URL.
pub fn ip_is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_blocked(v4),
        IpAddr::V6(v6) => ipv6_blocked(v6),
    }
}

fn ipv4_blocked(v4: Ipv4Addr) -> bool {
    let [a, b, c, _] = v4.octets();
    // 0.0.0.0/8 "this network": Linux and macOS dial 0.0.0.0 as this computer.
    a == 0
        || v4.is_private() // 10/8, 172.16/12, 192.168/16
        || v4.is_loopback() // 127/8
        || v4.is_link_local() // 169.254/16, cloud metadata (169.254.169.254) included
        || v4.is_documentation() // 192.0.2/24, 198.51.100/24, 203.0.113/24
        // 100.64.0.0/10: carrier-grade NAT, and Tailscale's default range.
        || (a == 100 && (b & 0xc0) == 0x40)
        // 192.0.0.0/24: IETF protocol assignments (DS-Lite, NAT64 discovery, two anycast
        // services). Nothing a feed or a mod fetches lives there, so all of it is refused.
        || (a == 192 && b == 0 && c == 0)
        // 198.18.0.0/15: network benchmarking, routed inside test labs.
        || (a == 198 && (b & 0xfe) == 18)
        // 224.0.0.0/4 multicast and 240.0.0.0/4 reserved, 255.255.255.255 broadcast included.
        // A feed or a mod never needs multicast.
        || a >= 224
}

fn ipv6_blocked(v6: Ipv6Addr) -> bool {
    // First, so they're refused as themselves, not as the 0.0.0.0 and 0.0.0.1 inside `::/96`.
    if v6.is_unspecified() || v6.is_loopback() {
        return true;
    }
    if let Some(v4) = embedded_ipv4(v6) {
        return ipv4_blocked(v4);
    }
    let [s0, s1, ..] = v6.segments();
    // Only 2000::/3 is global unicast. Outside it: fc00::/7 unique-local, fe80::/10 link-local,
    // fec0::/10 site-local, ff00::/8 multicast, 100::/64 discard, 64:ff9b:1::/48 local-use NAT64
    // (its operator picks where in the address the IPv4 sits, so it can't be read back out),
    // and every block not yet assigned.
    if (s0 & 0xe000) != 0x2000 {
        return true;
    }
    // 2001::/23 IETF protocol assignments: Teredo 2001::/32 (a tunnel to an IPv4 the address
    // hides), benchmarking 2001:2::/48, ORCHID. A few anycast services in it are global, but none is
    // something a feed or a mod fetches.
    (s0 == 0x2001 && s1 < 0x0200)
        // 2001:db8::/32 and 3fff::/20: documentation.
        || (s0 == 0x2001 && s1 == 0x0db8)
        || (s0 == 0x3fff && s1 < 0x1000)
}

/// The IPv4 address an IPv6 address carries, where the packet ends up at that IPv4:
/// - `::ffff:a.b.c.d`, IPv4-mapped: the OS dials the IPv4 itself.
/// - `::a.b.c.d`, IPv4-compatible (deprecated): tunnelled to the IPv4 where an old tunnel is up.
/// - `64:ff9b::a.b.c.d`, the well-known NAT64 prefix: a translator dials the IPv4.
/// - `2002:aabb:ccdd::/48`, 6to4: tunnelled to `aa.bb.cc.dd`.
fn embedded_ipv4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = v6.segments();
    let o = v6.octets();
    let tail = Ipv4Addr::new(o[12], o[13], o[14], o[15]);
    match s {
        [0, 0, 0, 0, 0, 0xffff, _, _] | [0, 0, 0, 0, 0, 0, _, _] => Some(tail),
        [0x0064, 0xff9b, 0, 0, 0, 0, _, _] => Some(tail),
        [0x2002, ..] => Some(Ipv4Addr::new(o[2], o[3], o[4], o[5])),
        _ => None,
    }
}

/// Screen a lookup's answers. Every one must be public: a resolver that mixes a public answer with
/// a private one is refused whole, or the client could dial the private one. And there must be at
/// least one.
fn screen_addrs(addrs: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, Refusal> {
    if addrs.is_empty() {
        return Err(Refusal::Unresolved);
    }
    if addrs.iter().any(|a| ip_is_blocked(a.ip())) {
        return Err(Refusal::NotPublic);
    }
    Ok(addrs)
}

/// Resolve `host` and screen every address, returning them all (with `port`) for the caller to pin
/// onto its client. An IP literal is screened as is, with no lookup. Fails closed: an unresolvable
/// host, or one with any non-public answer, is refused.
///
/// The lookup has no deadline of its own (the system resolver's timeouts apply): a caller that
/// needs one wraps this call in `tokio::time::timeout`.
pub async fn resolve_and_screen(host: &str, port: u16) -> Result<Vec<SocketAddr>, Refusal> {
    resolve_and_screen_with(host, port, system_lookup).await
}

/// `getaddrinfo` blocks, and tokio's own resolver needs its `net` feature, which PM doesn't enable,
/// so the lookup runs on the blocking pool rather than stalling an async worker.
fn system_lookup(host: String, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    (host.as_str(), port)
        .to_socket_addrs()
        .map(Iterator::collect)
}

/// [`resolve_and_screen`] with the lookup passed in, so the screening around it is tested without
/// a network.
async fn resolve_and_screen_with<L>(
    host: &str,
    port: u16,
    lookup: L,
) -> Result<Vec<SocketAddr>, Refusal>
where
    L: FnOnce(String, u16) -> std::io::Result<Vec<SocketAddr>> + Send + 'static,
{
    if let Some(ip) = literal_ip(host) {
        return screen_addrs(vec![SocketAddr::new(ip, port)]);
    }
    let host = host.to_owned();
    match tokio::task::spawn_blocking(move || lookup(host, port)).await {
        Ok(Ok(addrs)) => screen_addrs(addrs),
        // A failed lookup and a lookup task that panicked both fail closed.
        Ok(Err(_)) | Err(_) => Err(Refusal::Unresolved),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(ip: &str) -> bool {
        ip_is_blocked(ip.parse().unwrap())
    }

    #[track_caller]
    fn assert_blocked(ips: &[&str]) {
        for ip in ips {
            assert!(blocked(ip), "{ip} should be blocked");
        }
    }

    #[track_caller]
    fn assert_public(ips: &[&str]) {
        for ip in ips {
            assert!(!blocked(ip), "{ip} should be public");
        }
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn run<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    // --- the ranges the calendar screen already had (lifted unchanged) ---

    #[test]
    fn the_ranges_calendar_feeds_always_blocked_are_still_blocked() {
        assert_blocked(&[
            "127.0.0.1",
            "127.255.255.254",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "255.255.255.255",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "febf:ffff::1",
        ]);
        assert_public(&[
            "8.8.8.8",
            "93.184.216.34",
            "1.1.1.1",
            // The neighbours of the private ranges stay public.
            "9.255.255.255",
            "11.0.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "100.63.255.255",
            "100.128.0.0",
            "169.253.255.255",
            "126.255.255.255",
            "128.0.0.0",
        ]);
    }

    // --- the new IPv4 ranges ---

    #[test]
    fn the_whole_of_0_0_0_0_slash_8_is_blocked_not_just_0_0_0_0() {
        assert_blocked(&["0.0.0.1", "0.1.2.3", "0.255.255.255"]);
        assert_public(&["1.0.0.0"]);
    }

    #[test]
    fn benchmarking_198_18_slash_15_is_blocked() {
        assert_blocked(&["198.18.0.0", "198.18.0.1", "198.19.255.255"]);
        assert_public(&["198.17.255.255", "198.20.0.0"]);
    }

    #[test]
    fn reserved_240_slash_4_is_blocked() {
        assert_blocked(&["240.0.0.0", "250.1.2.3", "255.255.255.254"]);
    }

    #[test]
    fn ipv4_multicast_224_slash_4_is_blocked() {
        assert_blocked(&[
            "224.0.0.0",
            "224.0.0.251",
            "239.255.255.250",
            "239.255.255.255",
        ]);
        assert_public(&["223.255.255.255"]);
    }

    #[test]
    fn ietf_protocol_assignments_192_0_0_slash_24_are_blocked() {
        // 192.0.0.170 is NAT64 discovery; 192.0.0.9 is an anycast PM has no use for.
        assert_blocked(&["192.0.0.0", "192.0.0.9", "192.0.0.170", "192.0.0.255"]);
        assert_public(&["191.255.255.255", "192.0.1.0"]);
    }

    // --- the new IPv6 ranges ---

    #[test]
    fn ipv6_multicast_ff00_slash_8_is_blocked() {
        assert_blocked(&["ff00::", "ff02::1", "ff05::1:3", "ff0e::1", "ffff:ffff::1"]);
    }

    #[test]
    fn site_local_fec0_slash_10_is_blocked() {
        assert_blocked(&["fec0::", "fec0::1", "feff:ffff:ffff:ffff::1"]);
    }

    #[test]
    fn the_well_known_nat64_prefix_is_screened_as_the_ipv4_it_carries() {
        assert_blocked(&[
            "64:ff9b::7f00:1",    // 127.0.0.1
            "64:ff9b::127.0.0.1", // the same, dotted
            "64:ff9b::c0a8:101",  // 192.168.1.1
            "64:ff9b::a9fe:a9fe", // 169.254.169.254
            "64:ff9b::a00:1",     // 10.0.0.1
            "64:ff9b::",          // 0.0.0.0
            "64:ff9b::c612:1",    // 198.18.0.1
            "64:ff9b::e000:fb",   // 224.0.0.251
        ]);
        // On an IPv6-only network with DNS64, every IPv4-only site looks like this: a public
        // embedded address stays reachable.
        assert_public(&["64:ff9b::808:808", "64:ff9b::93.184.216.34"]);
        // Inside 64:ff9b::/32 but outside the /96 (bits set between the prefix and the IPv4) it is
        // no well-known form at all, so it's refused.
        assert_blocked(&["64:ff9b:0:0:1::808:808", "64:ff9b:2::808:808"]);
    }

    #[test]
    fn the_local_use_nat64_prefix_is_blocked_whatever_it_carries() {
        // Its operator picks the prefix length (/48 to /96), so where the IPv4 sits can't be read
        // from the address alone: even a public-looking tail is refused.
        assert_blocked(&[
            "64:ff9b:1::",
            "64:ff9b:1::808:808",
            "64:ff9b:1::7f00:1",
            "64:ff9b:1:ffff:ffff:ffff:ffff:ffff",
        ]);
    }

    #[test]
    fn six_to_four_is_screened_as_the_ipv4_it_carries() {
        assert_blocked(&[
            "2002:7f00:1::1",   // 127.0.0.1
            "2002:c0a8:101::1", // 192.168.1.1
            "2002:a9fe:a9fe::", // 169.254.169.254
            "2002:a00:1:5::1",  // 10.0.0.1
            "2002::1",          // 0.0.0.0
            "2002:e000:1::1",   // 224.0.0.1
        ]);
        assert_public(&["2002:808:808::1", "2002:5db8:d822:1::1"]);
    }

    #[test]
    fn ipv4_mapped_addresses_are_screened_as_their_ipv4() {
        assert_blocked(&[
            "::ffff:127.0.0.1",
            "::ffff:7f00:1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:0.0.0.0",
            "::ffff:198.18.0.1",
            "::ffff:240.0.0.1",
        ]);
        assert_public(&["::ffff:8.8.8.8", "::ffff:93.184.216.34"]);
    }

    #[test]
    fn ipv4_compatible_addresses_are_screened_as_their_ipv4() {
        assert_blocked(&[
            "::127.0.0.1",
            "::10.0.0.1",
            "::192.168.1.1",
            "::169.254.169.254",
            "::0.0.0.2",
            "::224.0.0.1",
        ]);
        assert_public(&["::8.8.8.8"]);
    }

    #[test]
    fn only_global_unicast_ipv6_is_public() {
        assert_public(&[
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "2a00:1450:4009:81f::200e",
            "2001:200::1",  // the first address past 2001::/23
            "3fff:1000::1", // the first address past 3fff::/20
            "3ffe::1",
        ]);
        assert_blocked(&[
            "100::", // discard-only
            "100::1",
            "1fff:ffff::1",   // below 2000::/3
            "4000::1",        // above it
            "5f00::1",        // SRv6 SIDs
            "::ffff:0:a00:1", // the old IPv4-translated form, not mapped
            "2001::1",        // Teredo
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:2::1",        // benchmarking
            "2001:1ff:ffff::1", // the end of 2001::/23
            "2001:db8::1",      // documentation
            "2001:db8:ffff::1",
            "3fff::1", // documentation (2024)
            "3fff:fff:ffff::1",
        ]);
    }

    // --- literals ---

    #[test]
    fn literal_ip_reads_bare_and_bracketed_literals_and_nothing_else() {
        assert_eq!(
            literal_ip("93.184.216.34"),
            Some("93.184.216.34".parse().unwrap())
        );
        assert_eq!(literal_ip("[::1]"), Some("::1".parse().unwrap()));
        assert_eq!(literal_ip("::1"), Some("::1".parse().unwrap()));
        assert_eq!(literal_ip("example.com"), None);
        // Only ONE pair of brackets is a literal; anything else goes to the resolver, which refuses it.
        assert_eq!(literal_ip("[[::1]]"), None);
        assert_eq!(literal_ip("[::1"), None);
        assert_eq!(literal_ip(""), None);
    }

    // --- the resolver edge ---

    fn no_lookup(_: String, _: u16) -> std::io::Result<Vec<SocketAddr>> {
        panic!("an IP literal must never reach the resolver")
    }

    #[test]
    fn a_literal_is_pinned_as_is_with_no_lookup() {
        assert_eq!(
            run(resolve_and_screen_with("93.184.216.34", 443, no_lookup)),
            Ok(vec![sa("93.184.216.34:443")])
        );
        assert_eq!(
            run(resolve_and_screen_with(
                "[2606:4700:4700::1111]",
                8443,
                no_lookup
            )),
            Ok(vec![sa("[2606:4700:4700::1111]:8443")])
        );
    }

    #[test]
    fn a_blocked_literal_is_refused_with_no_lookup() {
        for host in [
            "127.0.0.1",
            "10.0.0.5",
            "169.254.169.254",
            "[::1]",
            "[64:ff9b::7f00:1]",
            "[2002:c0a8:101::1]",
            "[::ffff:127.0.0.1]",
            "0.0.0.0",
        ] {
            assert_eq!(
                run(resolve_and_screen_with(host, 443, no_lookup)),
                Err(Refusal::NotPublic),
                "{host}"
            );
        }
    }

    #[test]
    fn a_name_with_only_public_answers_is_pinned_to_all_of_them_in_order() {
        let got = run(resolve_and_screen_with(
            "feed.example",
            443,
            |host, port| {
                assert_eq!((host.as_str(), port), ("feed.example", 443));
                Ok(vec![
                    sa("93.184.216.34:443"),
                    sa("[2606:2800:220:1::1]:443"),
                ])
            },
        ));
        assert_eq!(
            got,
            Ok(vec![
                sa("93.184.216.34:443"),
                sa("[2606:2800:220:1::1]:443")
            ])
        );
    }

    #[test]
    fn one_private_answer_refuses_the_whole_name() {
        // A rebinding resolver that mixes a public answer with a private one must not get the
        // private one dialled.
        for bad in [
            "127.0.0.1:443",
            "[64:ff9b::a00:1]:443",
            "[fe80::1]:443",
            "198.18.0.1:443",
        ] {
            let got = run(resolve_and_screen_with("feed.example", 443, move |_, _| {
                Ok(vec![sa("93.184.216.34:443"), sa(bad)])
            }));
            assert_eq!(got, Err(Refusal::NotPublic), "{bad}");
        }
    }

    #[test]
    fn no_answer_a_failed_lookup_or_a_panicked_one_is_unresolved() {
        assert_eq!(
            run(resolve_and_screen_with("feed.example", 443, |_, _| Ok(
                vec![]
            ))),
            Err(Refusal::Unresolved)
        );
        assert_eq!(
            run(resolve_and_screen_with("feed.example", 443, |_, _| {
                Err(std::io::Error::other("NXDOMAIN"))
            })),
            Err(Refusal::Unresolved)
        );
        assert_eq!(
            run(resolve_and_screen_with(
                "feed.example",
                443,
                |_, _| -> std::io::Result<Vec<SocketAddr>> { panic!("resolver crashed") }
            )),
            Err(Refusal::Unresolved)
        );
    }

    #[test]
    fn the_system_resolver_is_screened_too() {
        // `localhost` resolves from the hosts file, with no DNS query.
        assert_eq!(
            run(resolve_and_screen("localhost", 443)),
            Err(Refusal::NotPublic)
        );
        // getaddrinfo rejects a name with spaces locally, so this issues no DNS query either.
        assert_eq!(
            run(resolve_and_screen("not a host", 443)),
            Err(Refusal::Unresolved)
        );
        // A literal still skips the resolver.
        assert_eq!(
            run(resolve_and_screen("93.184.216.34", 443)),
            Ok(vec![sa("93.184.216.34:443")])
        );
    }
}
