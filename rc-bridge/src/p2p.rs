//! P2P 打洞：用信令下发的候选地址打一条直连 QUIC（`RCP2` + BiDi mTLS）。
//!
//! 选路顺序是「局域网候选 → 公网候选」由 `rc-client-core` 的 `parse_candidate_addrs`
//! 决定；打洞成功后拿到的是完整 QUIC 传输（datagram 媒体 + reliable 控制 + urgent 输入），
//! 与中继那条路在会话层看来完全一样。
//!
//! 打洞失败（对端没有候选地址、NAT 打不通、超时）就由调用方退回中继。

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use rc_client_core::p2p::{attempt_p2p_quic_client_mtls, attempt_p2p_quic_host_mtls, P2pCandidates};
use rc_client_core::relay_transport::SessionMtlsBundle;
use rc_net::Transport;

/// 采用 Core 与原生客户端相同的跨公网 LAN 候选过滤，再交由 Core 排序和打洞。
pub fn filter_candidates(mut candidates: P2pCandidates, self_public: Option<&str>) -> P2pCandidates {
    let local: Vec<_> = self_public.into_iter().collect();
    let peer: Vec<_> = candidates.peer_public_addr.as_deref().into_iter()
        .chain(candidates.peer_public_addrs.iter().map(String::as_str)).collect();
    candidates.peer_lan_addrs = rc_client_core::p2p::filter_cross_site_lan_addrs(
        &candidates.peer_lan_addrs, &local, &peer,
    );
    eprintln!("rc-bridge P2P candidates: session={} local={:?} public={:?}/{:?} lan={:?} nat={:?}/{:?} start={:?} hmac={}",
        candidates.session_id, candidates.local_udp_port, candidates.peer_public_addr,
        candidates.peer_public_addrs, candidates.peer_lan_addrs, candidates.self_nat_type,
        candidates.peer_nat_type, candidates.start_after_ms, candidates.punch_secret.is_some());
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_site_filter_preserves_public_candidates_and_nat_hints() {
        let mut candidates = P2pCandidates::default();
        candidates.peer_public_addr = Some("203.0.113.9:5000".into());
        candidates.peer_public_addrs = vec!["[2606:4700::1]:5001".into()];
        candidates.peer_lan_addrs = vec!["192.168.1.1:5000".into()];
        candidates.peer_nat_port_delta = Some(2);
        candidates.self_nat_port_delta = Some(4);
        let filtered = filter_candidates(candidates, Some("198.51.100.2:4000"));
        assert!(filtered.peer_lan_addrs.is_empty());
        assert_eq!(filtered.peer_public_addrs, ["[2606:4700::1]:5001"]);
        assert_eq!((filtered.self_nat_port_delta, filtered.peer_nat_port_delta), (Some(4), Some(2)));
    }
}

/// 一次打洞的时间预算。
pub struct PunchBudget {
    /// 打洞（UDP 探测对齐）上限。
    pub punch: Duration,
    /// 打洞成功后的 QUIC 握手上限。
    pub quic: Duration,
}

impl Default for PunchBudget {
    fn default() -> Self {
        Self {
            punch: rc_client_core::p2p::DEFAULT_PUNCH_TIMEOUT,
            quic: rc_client_core::p2p::DEFAULT_QUIC_HANDSHAKE_TIMEOUT,
        }
    }
}

/// 控制端打洞。
///
/// @param candidates - 信令下发的候选地址。
/// @param mtls - 双向 mTLS 材料（含预期对端）。
/// @param budget - 时间预算。
pub fn punch_client(
    candidates: &P2pCandidates,
    mtls: &SessionMtlsBundle,
    budget: &PunchBudget,
) -> Result<Box<dyn Transport>, String> {
    let config = mtls.to_quic_mtls_config()?;
    let cancel = AtomicBool::new(false);
    let port_hold = rc_client_core::signal_client::acquire_local_udp_port_for_p2p(candidates.local_udp_port);
    let connection = attempt_p2p_quic_client_mtls(
        candidates,
        budget.punch,
        budget.quic,
        &cancel,
        Some(&config),
    )?;
    eprintln!(
        "rc-bridge P2P：控制端打洞成功，对端 {}（RTT {}µs），mTLS 身份 {}",
        connection.peer_addr,
        connection.rtt_us,
        if connection.peer_mtls_identity().is_some() { "已验证" } else { "缺失" }
    );
    Ok(Box::new(rc_client_core::signal_client::P2pHeldTransport::new(connection, port_hold)))
}

/// 被控端打洞。
///
/// @param candidates - 信令下发的候选地址。
/// @param mtls - 双向 mTLS 材料（含预期对端）。
/// @param budget - 时间预算。
pub fn punch_host(
    candidates: &P2pCandidates,
    mtls: &SessionMtlsBundle,
    budget: &PunchBudget,
) -> Result<Box<dyn Transport>, String> {
    let config = mtls.to_quic_mtls_config()?;
    let cancel = AtomicBool::new(false);
    let port_hold = rc_client_core::signal_client::acquire_local_udp_port_for_p2p(candidates.local_udp_port);
    let connection = attempt_p2p_quic_host_mtls(
        candidates,
        budget.punch,
        budget.quic,
        &cancel,
        Some(&config),
    )?;
    eprintln!(
        "rc-bridge P2P：被控端打洞成功，对端 {}（RTT {}µs），mTLS 身份 {}",
        connection.peer_addr,
        connection.rtt_us,
        if connection.peer_mtls_identity().is_some() { "已验证" } else { "缺失" }
    );
    Ok(Box::new(rc_client_core::signal_client::P2pHeldTransport::new(connection, port_hold)))
}
