//! A stand-in for netd's `dnsproxyd`. bionic's resolver (via libnetd_client)
//! proxies DNS to `/dev/socket/dnsproxyd`; arod shares the host network
//! namespace, so it answers by resolving on the host — the app's DNS is the
//! desktop's DNS, no netd. Implements the `getaddrinfo`/`gethostbyname`/
//! `gethostbyaddr` commands bionic sends (see bionic getaddrinfo.c).
use std::io::{Read, Write};
use std::net::{IpAddr, ToSocketAddrs};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

// netd ResponseCode::DnsProxyQueryResult
const DNS_PROXY_QUERY_RESULT: &[u8; 4] = b"222\0";

pub fn serve(sock_dir: &Path) -> std::io::Result<()> {
    let path = sock_dir.join("dnsproxyd");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o666))?;
    std::thread::Builder::new().name("aro-dnsproxyd".into()).spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    std::thread::spawn(move || {
                        if let Err(e) = handle(s) {
                            log::debug!("dnsproxyd: {e}");
                        }
                    });
                }
                Err(e) => log::debug!("dnsproxyd: accept: {e}"),
            }
        }
    })?;
    log::info!("dnsproxyd: serving {}", path.display());
    Ok(())
}

fn read_request(s: &mut UnixStream) -> std::io::Result<Vec<String>> {
    let mut buf = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        let n = s.read(&mut byte)?;
        if n == 0 {
            break;
        }
        if byte[0] == 0 {
            break;
        }
        buf.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&buf).split(' ').map(str::to_string).collect())
}

fn be32(v: i32) -> [u8; 4] {
    v.to_be_bytes()
}

/// sockaddr bytes as bionic reads them into sockaddr_storage (native layout).
fn sockaddr_bytes(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(a) => {
            let mut b = Vec::with_capacity(16);
            b.extend_from_slice(&(libc::AF_INET as u16).to_ne_bytes()); // sin_family
            b.extend_from_slice(&0u16.to_be_bytes()); // sin_port
            b.extend_from_slice(&a.octets()); // sin_addr (network order)
            b.extend_from_slice(&[0u8; 8]); // sin_zero
            b
        }
        IpAddr::V6(a) => {
            let mut b = Vec::with_capacity(28);
            b.extend_from_slice(&(libc::AF_INET6 as u16).to_ne_bytes()); // sin6_family
            b.extend_from_slice(&0u16.to_be_bytes()); // sin6_port
            b.extend_from_slice(&0u32.to_be_bytes()); // sin6_flowinfo
            b.extend_from_slice(&a.octets()); // sin6_addr
            b.extend_from_slice(&0u32.to_ne_bytes()); // sin6_scope_id
            b
        }
    }
}

fn write_addr(out: &mut Vec<u8>, ip: IpAddr, socktype: i32, protocol: i32) {
    let sa = sockaddr_bytes(ip);
    let family = match ip {
        IpAddr::V4(_) => libc::AF_INET,
        IpAddr::V6(_) => libc::AF_INET6,
    };
    out.extend_from_slice(&be32(1)); // have_more
    out.extend_from_slice(&be32(0)); // ai_flags
    out.extend_from_slice(&be32(family)); // ai_family
    out.extend_from_slice(&be32(if socktype > 0 { socktype } else { libc::SOCK_STREAM })); // ai_socktype
    out.extend_from_slice(&be32(if protocol > 0 { protocol } else { libc::IPPROTO_TCP })); // ai_protocol
    out.extend_from_slice(&be32(sa.len() as i32)); // ai_addrlen
    out.extend_from_slice(&sa); // sockaddr
    out.extend_from_slice(&be32(0)); // canonname length (none)
}

fn handle(mut s: UnixStream) -> std::io::Result<()> {
    let argv = read_request(&mut s)?;
    let Some(cmd) = argv.first() else { return Ok(()) };
    match cmd.as_str() {
        // getaddrinfo host serv flags family socktype protocol netid
        "getaddrinfo" => {
            let host = argv.get(1).map(String::as_str).unwrap_or("^");
            let family: i32 = argv.get(4).and_then(|s| s.parse().ok()).unwrap_or(-1);
            let socktype: i32 = argv.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
            let protocol: i32 = argv.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
            if host == "^" {
                return fail(&mut s);
            }
            let mut out = Vec::new();
            out.extend_from_slice(DNS_PROXY_QUERY_RESULT);
            let mut count = 0;
            match (host, 0u16).to_socket_addrs() {
                Ok(addrs) => {
                    for sa in addrs {
                        let ip = sa.ip();
                        let want = match family {
                            2 => ip.is_ipv4(),   // AF_INET
                            10 => ip.is_ipv6(),  // AF_INET6
                            _ => true,           // AF_UNSPEC
                        };
                        if want {
                            write_addr(&mut out, ip, socktype, protocol);
                            count += 1;
                        }
                    }
                }
                Err(e) => {
                    log::debug!("dnsproxyd: resolve {host}: {e}");
                }
            }
            out.extend_from_slice(&be32(0)); // end of list
            log::info!("dnsproxyd: getaddrinfo {host} -> {count} address(es)");
            s.write_all(&out)?;
        }
        // gethostbyname netid name af
        "gethostbyname" => {
            let name = argv.get(2).map(String::as_str).unwrap_or("^");
            let mut out = Vec::new();
            out.extend_from_slice(DNS_PROXY_QUERY_RESULT);
            // hostent wire form: BE32 name len + name, BE32 addrtype, BE32 addrlen,
            // then BE32 count + each address; kept minimal (unused by HttpUrlConnection).
            if name != "^" {
                if let Ok(addrs) = (name, 0u16).to_socket_addrs() {
                    let v4: Vec<_> = addrs.filter_map(|a| match a.ip() { IpAddr::V4(x) => Some(x), _ => None }).collect();
                    if !v4.is_empty() {
                        let nb = name.as_bytes();
                        out.extend_from_slice(&be32(nb.len() as i32 + 1));
                        out.extend_from_slice(nb);
                        out.push(0);
                        out.extend_from_slice(&be32(libc::AF_INET));
                        out.extend_from_slice(&be32(4));
                        out.extend_from_slice(&be32(v4.len() as i32));
                        for a in v4 {
                            out.extend_from_slice(&a.octets());
                        }
                        s.write_all(&out)?;
                        return Ok(());
                    }
                }
            }
            out.extend_from_slice(&be32(0));
            s.write_all(&out)?;
        }
        other => {
            log::debug!("dnsproxyd: unhandled command {other:?}");
            fail(&mut s)?;
        }
    }
    Ok(())
}

fn fail(s: &mut UnixStream) -> std::io::Result<()> {
    // Non-success code so the client returns EAI_NODATA.
    s.write_all(b"223\0")?;
    Ok(())
}
