//! aro-exec: the ARO process host.
//!
//! M1: runs Android's `app_process64` inside an unprivileged namespace built
//! from the unpacked AOSP image, with the in-VM bootstrap that loads an
//! unmodified APK through ART and invokes a static method.
use anyhow::{bail, Result};
use aro_exec::{layout::Layout, logd, ns, prepare};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aro-exec", about = "ARO process host")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Regenerate derived state (properties, apex list, aconfig, linker config, classpath)
    Prepare,
    /// Load an APK in ART and invoke a static method (M1), or run it as an app (--app)
    Run {
        apk: PathBuf,
        class: Option<String>,
        method: Option<String>,
        /// Use dalvikvm64 directly instead of app_process64 (no Android runtime init)
        #[arg(long)]
        bare: bool,
        /// Enter android.app.ActivityThread.main (needs an arod session with services)
        #[arg(long)]
        app: bool,
        /// Debug: call a static no-arg method of a boot-classpath class: --call CLASS METHOD
        #[arg(long, num_args = 2)]
        call: Option<Vec<String>>,
    },
    /// Run an arbitrary command inside the namespace (debugging)
    Shell {
        #[arg(default_value = "/system/bin/sh")]
        argv: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let layout = Layout::default();
    if !layout.system.join("system/bin/app_process64").exists() {
        bail!("no unpacked system image at {} (run: aro-image unpack)", layout.system.display());
    }
    match cli.cmd {
        Cmd::Prepare => {
            let _ = std::fs::remove_file(layout.state.join("linkerconfig/ld.config.txt"));
            let _ = std::fs::remove_file(layout.data.join("local/tmp/classpath.env"));
            prepare::prepare(&layout)
        }
        Cmd::Run { apk, class, method, bare, app, call } => {
            if !layout.state.join("props").exists() {
                prepare::prepare(&layout)?;
            }
            if !layout.bootstrap_jar.exists() {
                bail!("bootstrap jar missing at {}", layout.bootstrap_jar.display());
            }
            let apk = apk.canonicalize()?;
            let name = apk.file_name().unwrap().to_string_lossy().into_owned();
            let apk_in = format!("/data/local/tmp/{name}");
            let mut argv = if bare {
                vec!["/apex/com.android.art/bin/dalvikvm64".into(), "-cp".into(), "/data/local/tmp/aro-bootstrap.jar".into(), "org.aro.Bootstrap".into()]
            } else {
                vec!["/system/bin/app_process64".into(), "/system/bin".into(), "org.aro.Bootstrap".into()]
            };
            if let Some(c) = call {
                argv.push("--call".into());
                argv.extend(c);
            } else if app {
                argv.push("--app".into());
            } else {
                argv.push(apk_in);
                argv.push(class.ok_or_else(|| anyhow::anyhow!("class required (or --app)"))?);
                if let Some(m) = method {
                    argv.push(m);
                }
            }
            let env = vec![("CLASSPATH".to_string(), "/data/local/tmp/aro-bootstrap.jar".to_string())];
            exec(&layout, ns::Spec { layout: &layout, extra_files: vec![(apk, name)], env, argv, session: ns::Session::from_env() })
        }
        Cmd::Shell { argv } => {
            if !layout.state.join("props").exists() {
                prepare::prepare(&layout)?;
            }
            exec(&layout, ns::Spec { layout: &layout, extra_files: vec![], env: vec![], argv, session: ns::Session::from_env() })
        }
    }
}

fn exec(layout: &Layout, spec: ns::Spec) -> Result<()> {
    // No threads: unshare(CLONE_NEWUSER) needs a single-threaded process and
    // thread creation is impossible after unshare(CLONE_NEWPID). The parent
    // pumps the log socket while it waits for the child. Inside an arod
    // session the supervisor owns the log socket instead.
    let sock = match &spec.session {
        Some(_) => None,
        None => {
            let s = logd::bind(&layout.runtime.join("socket"))?;
            s.set_nonblocking(true)?;
            Some(s)
        }
    };
    let code = ns::run(&spec, sock.as_ref())?;
    std::process::exit(code);
}
