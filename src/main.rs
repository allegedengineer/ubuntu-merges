#![warn(clippy::pedantic)]

use clap::Parser;
use clap_verbosity_flag::InfoLevel;
use tokio::runtime::Runtime;
use tracing_indicatif::IndicatifLayer;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

mod grab;
mod ls;
mod init;

#[derive(clap::Parser)]
struct Args {
    #[command(flatten)]
    verbosity: clap_verbosity_flag::Verbosity<InfoLevel>,
    #[command(subcommand)]
    verb: Verb,
}

#[derive(clap::Subcommand)]
enum Verb {
    Grab(grab::Grab),
    Ls(ls::Ls),
    Init(init::Init),
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();
    let indicatif_layer = IndicatifLayer::new();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(indicatif_layer.get_stderr_writer())
                .with_filter(args.verbosity.tracing_level_filter()),
        )
        .with(indicatif_layer.with_filter(args.verbosity.tracing_level_filter()))
        .init();
    let runtime = Runtime::new()?;
    runtime.block_on(async move {
        match &args.verb {
            Verb::Grab(grab) => grab.run(&args).await,
            Verb::Ls(ls) => ls.run(&args).await,
            Verb::Init(init) => init.run(&args).await,
        }
    })
}
