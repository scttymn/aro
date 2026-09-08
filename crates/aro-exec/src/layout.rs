//! Where ARO keeps things on the host.
use std::path::PathBuf;

pub struct Layout {
    /// Unpacked AOSP image root (from `aro-image unpack`).
    pub system: PathBuf,
    /// Android `/data` (persistent).
    pub data: PathBuf,
    /// Derived, regenerable state: properties, linker config, aconfig, classpath.
    pub state: PathBuf,
    /// Runtime files: sockets, the new root mount point.
    pub runtime: PathBuf,
    /// The in-VM bootstrap jar.
    pub bootstrap_jar: PathBuf,
}

impl Layout {
    pub fn default() -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or(home.join(".local/share"));
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or(std::env::temp_dir());
        let base = data_home.join("aro");
        Layout {
            system: base.join("system"),
            data: base.join("data"),
            state: base.join("state"),
            runtime: runtime_dir.join("aro"),
            bootstrap_jar: base.join("aro-bootstrap.jar"),
        }
    }
}
