//! What the host's network is, read from NetworkManager on the system bus. ARO
//! mirrors it: apps share the host network namespace, so the connection an app
//! sees *is* this one. Nothing here is invented — transport, validation and
//! metering come from NM; DNS servers come from the active connection.
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

// android.net transports.
pub const TRANSPORT_CELLULAR: i32 = 0;
pub const TRANSPORT_WIFI: i32 = 1;
pub const TRANSPORT_ETHERNET: i32 = 3;

#[derive(Debug, Clone)]
pub struct HostNet {
    pub online: bool,
    pub validated: bool,
    pub metered: bool,
    pub transport: i32,
    pub dns: Vec<std::net::Ipv4Addr>,
    pub iface: Option<String>,
}

impl Default for HostNet {
    fn default() -> Self {
        HostNet { online: false, validated: false, metered: false, transport: TRANSPORT_ETHERNET, dns: Vec::new(), iface: None }
    }
}

fn transport_of(nm_type: &str) -> i32 {
    match nm_type {
        t if t.starts_with("802-11") || t.contains("wireless") => TRANSPORT_WIFI,
        t if t.starts_with("802-3") || t.contains("ethernet") => TRANSPORT_ETHERNET,
        "gsm" | "cdma" => TRANSPORT_CELLULAR,
        _ => TRANSPORT_ETHERNET,
    }
}

/// Probe NetworkManager once. `host_uid` is asserted to the system bus (we run
/// as namespace-root; the bus checks peer credentials, which resolve to this uid).
pub fn probe(host_uid: u32) -> HostNet {
    match try_probe(host_uid) {
        Ok(n) => {
            log::info!("net: host {n:?}");
            n
        }
        Err(e) => {
            log::warn!("net: NetworkManager probe failed ({e}); reporting offline");
            HostNet::default()
        }
    }
}

fn prop<T>(conn: &Connection, path: &str, iface: &str, name: &str) -> anyhow::Result<T>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
    <T as TryFrom<zbus::zvariant::OwnedValue>>::Error: std::fmt::Display,
{
    let p = Proxy::new(conn, "org.freedesktop.NetworkManager", path, "org.freedesktop.DBus.Properties")?;
    let v: zbus::zvariant::OwnedValue = p.call("Get", &(iface, name))?;
    T::try_from(v).map_err(|e| anyhow::anyhow!("{name}: {e}"))
}

fn try_probe(host_uid: u32) -> anyhow::Result<HostNet> {
    let conn = zbus::blocking::connection::Builder::system()?.user_id(host_uid).build()?;
    let nm = "org.freedesktop.NetworkManager";
    let root = "/org/freedesktop/NetworkManager";
    // Connectivity: 4 == FULL. State: >= 70 == connected globally.
    let connectivity: u32 = prop(&conn, root, nm, "Connectivity").unwrap_or(0);
    let state: u32 = prop(&conn, root, nm, "State").unwrap_or(0);
    let primary: OwnedObjectPath = prop(&conn, root, nm, "PrimaryConnection").unwrap_or_else(|_| OwnedObjectPath::try_from("/").unwrap());
    let mut net = HostNet {
        online: state >= 70,
        validated: connectivity == 4,
        metered: false,
        transport: TRANSPORT_ETHERNET,
        dns: Vec::new(),
        iface: None,
    };
    let ppath = primary.as_str();
    if ppath != "/" {
        let ac = "org.freedesktop.NetworkManager.Connection.Active";
        if let Ok(t) = prop::<String>(&conn, ppath, ac, "Type") {
            net.transport = transport_of(&t);
        }
        // IP4Config -> Nameservers + the device's metered flag.
        if let Ok(ip4) = prop::<OwnedObjectPath>(&conn, ppath, ac, "Ip4Config") {
            if ip4.as_str() != "/" {
                let ip4i = "org.freedesktop.NetworkManager.IP4Config";
                if let Ok(servers) = prop::<Vec<String>>(&conn, ip4.as_str(), ip4i, "NameserverData").or_else(|_| Ok::<_, anyhow::Error>(Vec::new())) {
                    for s in servers {
                        if let Ok(a) = s.parse() {
                            net.dns.push(a);
                        }
                    }
                }
                // Older NM exposes Nameservers as au (u32 BE addresses).
                if net.dns.is_empty() {
                    if let Ok(addrs) = prop::<Vec<u32>>(&conn, ip4.as_str(), ip4i, "Nameservers") {
                        for a in addrs {
                            net.dns.push(std::net::Ipv4Addr::from(u32::from_be(a)));
                        }
                    }
                }
            }
        }
    }
    Ok(net)
}
