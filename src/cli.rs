use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "akagi", about = "Akagi - Mahjong AI Assistant")]
pub struct Cli {
    #[arg(short, long, help = "Path to config.toml")]
    pub config: Option<PathBuf>,

    #[arg(short, long, default_value_t = 3000, help = "Web UI port on 127.0.0.1")]
    pub port: u16,

    #[arg(long, help = "Do not open the Web UI in the default browser")]
    pub no_open: bool,
}
