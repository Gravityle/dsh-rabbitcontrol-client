//! Relay-first session transport; a verified background P2P candidate upgrades in place.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};
use std::time::{Duration, Instant};
use rc_client_core::{p2p::{P2pCandidates, attempt_p2p_quic_client_mtls, attempt_p2p_quic_host_mtls}, relay_transport::{RelayRole, SessionMtlsBundle}, switchable_transport::{CandidateTransport, SwitchHandle, SwitchableTransport, TransportPathKind}};
use rc_net::{Role, Transport};

pub struct ActiveSwitch {
    handle: SwitchHandle,
    cancel: Arc<AtomicBool>,
}
impl ActiveSwitch {
    pub fn report(&self) {
        for event in self.handle.drain_path_events() {
            eprintln!("rc-bridge route={}", if event.kind == TransportPathKind::Relay { "relay" } else { "p2p" });
        }
    }
}
impl Drop for ActiveSwitch {
    fn drop(&mut self) { self.cancel.store(true, Ordering::Release); }
}

struct CancelOnError(Option<Arc<AtomicBool>>);
impl Drop for CancelOnError {
    fn drop(&mut self) { if let Some(cancel) = &self.0 { cancel.store(true, Ordering::Release); } }
}

pub fn connect(candidates: P2pCandidates, mtls: SessionMtlsBundle, relay_addr: &str, token: &str, role: RelayRole) -> Result<(Box<dyn Transport>, Option<ActiveSwitch>), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut guard = CancelOnError(Some(cancel.clone()));
    let bg_cancel = cancel.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    let bg_mtls = mtls.clone();
    let client = matches!(role, RelayRole::Client);
    std::thread::Builder::new().name("rc-bridge-p2p-candidate".into()).spawn(move || {
        let result: Result<CandidateTransport, String> = (|| {
            let config = bg_mtls.to_quic_mtls_config()?;
            let budget = crate::p2p::PunchBudget::default();
            let port_hold = rc_client_core::signal_client::acquire_local_udp_port_for_p2p(candidates.local_udp_port);
            let connection = if client {
                attempt_p2p_quic_client_mtls(&candidates, budget.punch, budget.quic, &bg_cancel, Some(&config))
            } else {
                attempt_p2p_quic_host_mtls(&candidates, budget.punch, budget.quic, &bg_cancel, Some(&config))
            }?;
            let fp = connection.peer_mtls_identity().map(|id| id.cert_fingerprint_sha256.clone());
            if fp.is_none() { return Err("P2P candidate missing verified peer identity".into()); }
            Ok(CandidateTransport { transport: Box::new(rc_client_core::signal_client::P2pHeldTransport::new(connection, port_hold)), kind: TransportPathKind::P2p, mtls_verified: true, peer_cert_fingerprint: fp })
        })();
        let _ = tx.send(result);
    }).map_err(|e| e.to_string())?;
    // Both dials are concurrent; only ClientHello selects the Host's active path.
    // This is essential for old P2P-first peers and prevents split-brain selection.
    let (relay_tx, relay_rx) = mpsc::sync_channel(1);
    let relay_addr = relay_addr.to_owned();
    let token = token.to_owned();
    let relay_mtls = mtls.clone();
    let relay_cancel = cancel.clone();
    std::thread::Builder::new().name("rc-bridge-relay-candidate".into()).spawn(move || {
        let config = relay_mtls.to_relay_mtls_config(None);
        let result = rc_client_core::relay_transport::TcpRelayTransport::connect_mtls_cancellable(
            &relay_addr, role, &token, Duration::from_secs(20), relay_cancel, &config)
            .map(|t| Box::new(t) as Box<dyn Transport>).map_err(|e| e.to_string());
        let _ = relay_tx.send(result);
    }).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut ready_p2p = None;
    let mut ready_relay = None;
    let mut p2p_buf = Vec::new();
    let mut relay_buf = Vec::new();
    let (initial, kind, prebuf) = loop {
        if started.elapsed() > Duration::from_secs(25) {
            cancel.store(true, Ordering::Release);
            return Err("auto transport selection timed out".into());
        }
        if ready_p2p.is_none() {
            if let Ok(result) = rx.try_recv() {
                match result { Ok(t) => ready_p2p = Some(t), Err(e) => eprintln!("rc-bridge background P2P: {e}") }
            }
        }
        if ready_relay.is_none() {
            if let Ok(result) = relay_rx.try_recv() {
                match result { Ok(t) => ready_relay = Some(t), Err(e) => eprintln!("rc-bridge background relay: {e}") }
            }
        }
        if client {
            // A short fast-P2P grace mirrors upstream; thereafter relay wins when ready.
            if ready_p2p.is_some() && started.elapsed() < Duration::from_millis(400) {
                break (ready_p2p.take().unwrap().transport, TransportPathKind::P2p, Vec::new());
            }
            if started.elapsed() >= Duration::from_millis(400) {
                if let Some(t) = ready_relay.take() { break (t, TransportPathKind::Relay, Vec::new()); }
            }
            if let Some(t) = ready_p2p.take() { break (t.transport, TransportPathKind::P2p, Vec::new()); }
        } else {
            if let Some(t) = &mut ready_p2p {
                if received_hello(&mut t.transport, &mut p2p_buf)? {
                    break (ready_p2p.take().unwrap().transport, TransportPathKind::P2p, p2p_buf);
                }
            }
            if let Some(t) = &mut ready_relay {
                if received_hello(t, &mut relay_buf)? { break (ready_relay.take().unwrap(), TransportPathKind::Relay, relay_buf); }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let (mut transport, handle) = SwitchableTransport::new(initial, if client { Role::Controller } else { Role::Host }, kind, true);
    transport.push_prebuffered_events(prebuf);
    eprintln!("rc-bridge route={}", if kind == TransportPathKind::Relay { "relay" } else { "p2p" });
    if kind == TransportPathKind::P2p {
        cancel.store(true, Ordering::Release);
        guard.0 = None;
        return Ok((Box::new(transport), Some(ActiveSwitch { handle, cancel })));
    }
    let worker_handle = handle.clone();
    let worker_cancel = cancel.clone();
    std::thread::Builder::new().name("rc-bridge-p2p-upgrade".into()).spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let candidate = if let Some(candidate) = ready_p2p { candidate } else { loop {
            if worker_cancel.load(Ordering::Acquire) || Instant::now() >= deadline { return; }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(candidate)) => break candidate,
                Ok(Err(error)) => { eprintln!("rc-bridge background P2P: {error}"); return; }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        };
        while !worker_cancel.load(Ordering::Acquire) && Instant::now() < deadline {
            if worker_handle.is_switch_capable() {
                let result = if client { worker_handle.initiate_switch(candidate) } else { worker_handle.accept_switch_candidate(candidate) };
                if let Err(error) = result { eprintln!("rc-bridge P2P upgrade: {error}"); }
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }).map_err(|e| e.to_string())?;
    guard.0 = None;
    Ok((Box::new(transport), Some(ActiveSwitch { handle, cancel })))
}

fn received_hello(transport: &mut Box<dyn Transport>, prebuf: &mut Vec<rc_net::NetEvent>) -> Result<bool, String> {
    let events = transport.poll(Duration::from_millis(1)).map_err(|e| e.to_string())?;
    let mut hello = false;
    for event in events {
        if let rc_net::NetEvent::Reliable(ref bytes) = event {
            if !bytes.starts_with(b"RCSW") { hello = true; }
        }
        if let rc_net::NetEvent::Datagram(ref bytes) = event {
            if bytes.len() == 14 && bytes.starts_with(b"RCSW") && bytes[5] == 1 {
                let mut ack = bytes.clone(); ack[5] = 2; let _ = transport.send_datagram(&ack);
            }
        }
        prebuf.push(event);
    }
    Ok(hello)
}
