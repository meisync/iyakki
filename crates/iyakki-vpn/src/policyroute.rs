//! Policy routing for the tunnel (F-VPN-08): the text of the `ip rule`, `ip route ... table` and nftables mark rules that send
//! chosen destinations or chosen user accounts through the tunnel interface while everything else keeps the normal default
//! route. Pure: nothing here runs `ip` or `nft`; the helper takes the argument lists. Not used by the shipped plans yet:
//! [`crate::plan`] explains that the aarch64 kernel lacks policy routing, so this is for kernels with `CONFIG_IP_MULTIPLE_TABLES`.
// SPDX-License-Identifier: Apache-2.0

use std::net::IpAddr;

use crate::error::VpnError;
use crate::net::Cidr;
use crate::plan::DNS_MARK;
use crate::wgnl::valid_ifname;

pub const MAX_NETS: usize = 256;
pub const MAX_UIDS: usize = 32;
/// Our own table in the `ip rule` list (the kernel's own are 253 to 255).
pub const DEFAULT_TABLE: u32 = 0x4956;
pub const DEFAULT_MARK: u32 = 0x4956_0002;
pub const DEFAULT_PRIORITY: u32 = 10_000;
pub const MARK_TABLE: &str = "iyakki_vpn_mark";

pub struct PolicyInput<'a> {
    pub ifname: &'a str,
    pub table: u32,
    pub mark: u32,
    /// The `ip rule` priority (smaller is looked at first; the main table is at 32766).
    pub priority: u32,
    /// Destinations that go through the tunnel.
    pub include: &'a [Cidr],
    /// Destinations that never do, even inside an included range (the LAN, the VPN server).
    pub exclude: &'a [Cidr],
    /// User accounts whose every connection goes through the tunnel (`meta skuid`).
    pub uids: &'a [u32],
    /// The application layer marks its own sockets (the reader browser and HTTP client, F-VPN-08): the rule and the table are put in
    /// place even when no range or account is chosen.
    pub app_sockets: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyPlan {
    /// `ip` argument lists to apply, in order, and to undo (reverse order).
    pub add: Vec<Vec<String>>,
    pub remove: Vec<Vec<String>>,
    /// The nftables text that marks the packets (table `inet iyakki_vpn_mark`).
    pub nft: String,
    mark: u32,
    table: u32,
    priority: u32,
    include: Vec<Cidr>,
    exclude: Vec<Cidr>,
    uids: Vec<u32>,
    app_sockets: bool,
}

fn v(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

pub fn plan_policy_routes(i: &PolicyInput) -> Result<PolicyPlan, VpnError> {
    if !valid_ifname(i.ifname) {
        return Err(VpnError::Unsafe("not a valid interface name".into()));
    }
    if !(1..=0x7fff_ffff).contains(&i.table) || (253..=255).contains(&i.table) {
        return Err(VpnError::Unsafe("the routing table number must not be one of the kernel's own (253 to 255) or 0".into()));
    }
    if i.mark == 0 || i.mark == DNS_MARK {
        return Err(VpnError::Unsafe("the packet mark must be set and different from the DNS forwarder's mark".into()));
    }
    if !(1..32766).contains(&i.priority) {
        return Err(VpnError::Unsafe("the rule priority must be between 1 and 32765 so it is looked at before the main table".into()));
    }
    if i.include.len() > MAX_NETS || i.exclude.len() > MAX_NETS || i.uids.len() > MAX_UIDS {
        return Err(VpnError::TooLarge("too many ranges or accounts in the tunnel routing".into()));
    }
    if i.include.is_empty() && i.uids.is_empty() && !i.app_sockets {
        return Err(VpnError::Missing("nothing is chosen to go through the tunnel".into()));
    }
    if i.uids.iter().any(|u| *u == 0) {
        return Err(VpnError::Unsafe("the administrator account (0) is never sent through the tunnel by account".into()));
    }
    let table = i.table.to_string();
    let (mark, prio) = (format!("{:#x}", i.mark), i.priority.to_string());
    let mut add = Vec::new();
    for fam in ["-4", "-6"] {
        add.push(v(&[fam, "rule", "add", "fwmark", &mark, "lookup", &table, "priority", &prio]));
        add.push(v(&[fam, "route", "add", "default", "dev", i.ifname, "table", &table]));
    }
    let remove: Vec<Vec<String>> = add.iter().rev().map(|a| a.iter().map(|s| if s == "add" { "del".to_string() } else { s.clone() }).collect()).collect();
    let include: Vec<Cidr> = i.include.iter().map(|c| c.network()).collect();
    let exclude: Vec<Cidr> = i.exclude.iter().map(|c| c.network()).collect();
    let set = |cs: &[Cidr], v4: bool| cs.iter().filter(|c| c.is_v4() == v4).map(|c| c.to_string()).collect::<Vec<_>>().join(", ");
    let mut nft = format!("table inet {MARK_TABLE} {{\n  chain output {{\n    type route hook output priority mangle; policy accept;\n    meta mark {:#x} return comment \"the DNS forwarder's own queries\"\n", DNS_MARK);
    for (fam, v4) in [("ip", true), ("ip6", false)] {
        let ex = set(&exclude, v4);
        if !ex.is_empty() {
            nft.push_str(&format!("    {fam} daddr {{ {ex} }} return comment \"never through the tunnel\"\n"));
        }
    }
    for (fam, v4) in [("ip", true), ("ip6", false)] {
        let inc = set(&include, v4);
        if !inc.is_empty() {
            nft.push_str(&format!("    {fam} daddr {{ {inc} }} meta mark set {mark} comment \"chosen destinations\"\n"));
        }
    }
    if !i.uids.is_empty() {
        let u = i.uids.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(", ");
        nft.push_str(&format!("    meta skuid {{ {u} }} meta mark set {mark} comment \"chosen accounts\"\n"));
    }
    nft.push_str("  }\n}\n");
    Ok(PolicyPlan { add, remove, nft, mark: i.mark, table: i.table, priority: i.priority, include, exclude, uids: i.uids.to_vec(), app_sockets: i.app_sockets })
}

impl PolicyPlan {
    pub fn mark(&self) -> u32 {
        self.mark
    }
    /// Does the application layer mark its own sockets for this plan?
    pub fn app_sockets(&self) -> bool {
        self.app_sockets
    }
    pub fn table(&self) -> u32 {
        self.table
    }
    pub fn priority(&self) -> u32 {
        self.priority
    }
    /// The destinations chosen to go through the tunnel (network form).
    pub fn include(&self) -> &[Cidr] {
        &self.include
    }
    pub fn exclude(&self) -> &[Cidr] {
        &self.exclude
    }
    pub fn uids(&self) -> &[u32] {
        &self.uids
    }
    /// Would a locally sent packet to `dst` from account `uid` (carrying `mark`) leave through the tunnel? Mirrors the nft text.
    pub fn goes_through_tunnel(&self, dst: IpAddr, uid: u32, mark: u32) -> bool {
        if mark == DNS_MARK {
            return false;
        }
        if self.exclude.iter().any(|c| c.contains_ip(dst)) {
            return false;
        }
        self.include.iter().any(|c| c.contains_ip(dst)) || self.uids.contains(&uid) || mark == self.mark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Cidr {
        Cidr::parse(s).unwrap()
    }
    fn input<'a>(inc: &'a [Cidr], exc: &'a [Cidr], uids: &'a [u32]) -> PolicyInput<'a> {
        PolicyInput { ifname: "wg0", table: DEFAULT_TABLE, mark: DEFAULT_MARK, priority: DEFAULT_PRIORITY, include: inc, exclude: exc, uids, app_sockets: false }
    }

    #[test]
    fn the_rule_route_and_mark_text_is_exactly_this() {
        let (inc, exc) = ([c("203.0.113.0/24"), c("2001:db8::/32")], [c("203.0.113.7/32")]);
        let p = plan_policy_routes(&input(&inc, &exc, &[1001])).unwrap();
        let joined: Vec<String> = p.add.iter().map(|a| a.join(" ")).collect();
        assert_eq!(
            joined,
            [
                "-4 rule add fwmark 0x49560002 lookup 18774 priority 10000",
                "-4 route add default dev wg0 table 18774",
                "-6 rule add fwmark 0x49560002 lookup 18774 priority 10000",
                "-6 route add default dev wg0 table 18774"
            ]
        );
        let undo: Vec<String> = p.remove.iter().map(|a| a.join(" ")).collect();
        assert_eq!(undo[0], "-6 route del default dev wg0 table 18774");
        assert_eq!(undo[3], "-4 rule del fwmark 0x49560002 lookup 18774 priority 10000");
        assert_eq!(
            p.nft,
            "table inet iyakki_vpn_mark {\n  chain output {\n    type route hook output priority mangle; policy accept;\n    meta mark 0x49560001 return comment \"the DNS forwarder's own queries\"\n    ip daddr { 203.0.113.7/32 } return comment \"never through the tunnel\"\n    ip daddr { 203.0.113.0/24 } meta mark set 0x49560002 comment \"chosen destinations\"\n    ip6 daddr { 2001:db8::/32 } meta mark set 0x49560002 comment \"chosen destinations\"\n    meta skuid { 1001 } meta mark set 0x49560002 comment \"chosen accounts\"\n  }\n}\n"
        );
    }

    #[test]
    fn the_evaluator_follows_the_same_order_as_the_rules() {
        let (inc, exc) = ([c("203.0.113.0/24")], [c("203.0.113.7/32")]);
        let p = plan_policy_routes(&input(&inc, &exc, &[1001])).unwrap();
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(p.goes_through_tunnel(ip("203.0.113.9"), 1000, 0));
        assert!(!p.goes_through_tunnel(ip("203.0.113.7"), 1000, 0), "the exclusion wins inside an included range");
        assert!(!p.goes_through_tunnel(ip("203.0.113.7"), 1001, 0), "and over an account choice");
        assert!(p.goes_through_tunnel(ip("198.51.100.1"), 1001, 0));
        assert!(!p.goes_through_tunnel(ip("198.51.100.1"), 1000, 0));
        assert!(!p.goes_through_tunnel(ip("203.0.113.9"), 1000, DNS_MARK), "the forwarder's own queries are left alone");
    }

    #[test]
    fn odd_input_is_refused_in_plain_words() {
        let inc = [c("203.0.113.0/24")];
        let bad = |f: &dyn Fn(&mut PolicyInput)| {
            let mut i = input(&inc, &[], &[]);
            f(&mut i);
            plan_policy_routes(&i).unwrap_err().to_string()
        };
        assert!(bad(&|i| i.ifname = "wg 0").contains("interface"));
        assert!(bad(&|i| i.table = 254).contains("table"));
        assert!(bad(&|i| i.table = 0).contains("table"));
        assert!(bad(&|i| i.mark = DNS_MARK).contains("mark"));
        assert!(bad(&|i| i.priority = 32766).contains("priority"));
        assert!(plan_policy_routes(&input(&[], &[], &[])).unwrap_err().to_string().contains("nothing"));
        assert!(plan_policy_routes(&input(&inc, &[], &[0])).unwrap_err().to_string().contains("administrator"));
        let many: Vec<Cidr> = (0..257).map(|n| c(&format!("10.{}.{}.0/24", n / 256, n % 256))).collect();
        assert!(plan_policy_routes(&input(&many, &[], &[])).unwrap_err().to_string().contains("too many"));
    }
}
