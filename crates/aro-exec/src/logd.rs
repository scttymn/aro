//! A stand-in for Android's logd: receives liblog datagrams on `logdw` and
//! prints them to stderr. Android code logs nothing to stdout/stderr; without
//! this, crashes and errors are invisible.
use anyhow::Result;
use std::os::unix::net::UnixDatagram;
use std::path::Path;

const PRI: [&str; 8] = ["?", "?", "V", "D", "I", "W", "E", "F"];

pub fn bind(sock_dir: &Path) -> Result<UnixDatagram> {
    std::fs::create_dir_all(sock_dir)?;
    let path = sock_dir.join("logdw");
    let _ = std::fs::remove_file(&path);
    let sock = UnixDatagram::bind(&path)?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o666))?;
    Ok(sock)
}

/// Block for one datagram, print it, then drain the rest. For a dedicated thread.
pub fn drain_blocking(sock: &UnixDatagram, buf: &mut [u8]) {
    if let Ok(n) = sock.recv(buf) {
        print_datagram(&buf[..n]);
    }
    drain(sock, buf);
}

/// Drain and print every datagram currently queued. Non-blocking.
pub fn drain(sock: &UnixDatagram, buf: &mut [u8]) {
    loop {
        let n = match sock.recv(buf) {
            Ok(n) => n,
            Err(_) => return,
        };
        print_datagram(&buf[..n]);
    }
}

fn print_datagram(d: &[u8]) {
    {
        let n = d.len();
        if n < 11 {
            return;
        }
        let log_id = d[0];
        let tid = u16::from_le_bytes([d[1], d[2]]);
        let p = &d[11..];
        if p.is_empty() {
            return;
        }
        if matches!(log_id, 0 | 1 | 3 | 4) {
            let pri = PRI.get(p[0] as usize).copied().unwrap_or("?");
            let rest = &p[1..];
            let (tag, msg) = match rest.iter().position(|&b| b == 0) {
                Some(i) => (&rest[..i], &rest[i + 1..]),
                None => (rest, &[][..]),
            };
            let msg = msg.strip_suffix(&[0]).unwrap_or(msg);
            eprintln!("{pri}/{}({tid}): {}", String::from_utf8_lossy(tag), String::from_utf8_lossy(msg).trim_end());
        }
    }
}
