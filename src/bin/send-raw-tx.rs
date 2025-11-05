use anyhow::Result;
use clap::Parser;
use gnostr_bitcoin::send_raw_tx;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// Hex-encoded raw transaction you’d like to blast
    #[arg(long)]
    tx: String,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let args = Args::parse();
    send_raw_tx::send_raw_transaction_to_peers(args.tx)
}