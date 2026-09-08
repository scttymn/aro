//! aro-props: generate a `/dev/__properties__` directory from build.prop files.
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aro-props")]
struct Cli {
    /// Output directory (becomes /dev/__properties__ in the namespace)
    #[arg(short, long)]
    out: PathBuf,
    /// build.prop-style files, in increasing priority
    #[arg(short = 'f', long = "file")]
    files: Vec<PathBuf>,
    /// Extra key=value pairs, highest priority
    #[arg(short = 's', long = "set")]
    sets: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut props = Vec::new();
    for f in &cli.files {
        let text = std::fs::read_to_string(f)?;
        props.extend(aro_props::parse_prop_file(&text));
    }
    for s in &cli.sets {
        if let Some((k, v)) = s.split_once('=') {
            props.push((k.to_string(), v.to_string()));
        }
    }
    let n = aro_props::write_properties_dir(&cli.out, &props)?;
    eprintln!("wrote {n} properties to {}", cli.out.display());
    Ok(())
}
