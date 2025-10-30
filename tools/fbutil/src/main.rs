use {
	clap::{Parser, Subcommand},
	url::Url,
};

mod watch;

#[tokio::main]
async fn main() -> eyre::Result<()> {
	color_eyre::install()?;
	let cli = Cli::parse();
	match &cli.command {
		Commands::Watch(args) => watch::run(&cli, args).await,
	}
}

#[derive(Debug, Clone, Parser)]
pub struct Cli {
	/// Flashblocks WebSocket endpoint address
	#[arg(
		long,
		short,
		default_value = "ws://localhost:1111",
		env = "WEBSOCKET_URL"
	)]
	ws: Url,

	/// Flashblocks builder eth RPC endpoint address
	#[arg(long, short, default_value = "http://localhost:2222", env = "RPC_URL")]
	rpc: Url,

	#[clap(subcommand)]
	command: Commands,
}

#[derive(Debug, Clone, Subcommand)]
enum Commands {
	Watch(watch::WatchArgs),
}
