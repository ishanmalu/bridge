//! Wake-on-LAN: switching to a sleeping machine wakes it.

use std::net::UdpSocket;

pub fn wake(macs: &[[u8; 6]]) {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return };
    let _ = sock.set_broadcast(true);
    for mac in macs {
        let mut packet = vec![0xFFu8; 6];
        for _ in 0..16 {
            packet.extend_from_slice(mac);
        }
        for port in [9, 7] {
            let _ = sock.send_to(&packet, ("255.255.255.255", port));
        }
    }
    if !macs.is_empty() {
        log::info!("sent Wake-on-LAN");
    }
}

/// Every network adapter's hardware address: the machine may be asleep on Ethernet or Wi-Fi.
pub fn own_macs() -> Vec<[u8; 6]> {
    let mut out: Vec<[u8; 6]> = Vec::new();
    if let Ok(iter) = mac_address::MacAddressIterator::new() {
        for m in iter {
            let b = m.bytes();
            if b != [0; 6] && !out.contains(&b) {
                out.push(b);
            }
        }
    }
    out.truncate(8);
    out
}
