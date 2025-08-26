//! Command line interface extensions for the Flashblocks builder.

use {
	crate::playground::PlaygroundOptions,
	clap::{CommandFactory, FromArgMatches, Parser},
	core::{net::SocketAddr, time::Duration},
	derive_more::{Deref, From, FromStr, Into},
	eyre::{Result, eyre},
	rblib::{
		alloy::signers::local::PrivateKeySigner,
		reth::optimism::{
			cli::{Cli as OpCli, chainspec::OpChainSpecParser, commands::Commands},
			node::args::RollupArgs,
		},
	},
	std::path::PathBuf,
};

pub type Cli = OpCli<OpChainSpecParser, BuilderArgs>;

#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[command(next_help_heading = "Rollup")]
pub struct BuilderArgs {
	/// Rollup configuration
	#[command(flatten)]
	pub rollup_args: RollupArgs,

	/// Whether to enable revert protection
	#[arg(
		long = "builder.revert-protection",
		default_value = "true",
		env = "REVERT_PROTECTION"
	)]
	pub revert_protection: bool,

	/// Builder secret key for signing last transaction in block
	#[arg(long = "rollup.builder-secret-key", env = "BUILDER_SECRET_KEY")]
	pub builder_signer: Option<BuilderSigner>,

	/// Path to builder playgorund to automatically start up the node connected
	/// to it
	#[arg(
        long = "builder.playground",
        num_args = 0..=1,
        default_missing_value = "$HOME/.playground/devnet/",
        value_parser = expand_path,
        env = "PLAYGROUND_DIR",
    )]
	pub playground: Option<PathBuf>,

	#[command(flatten)]
	pub flashblocks_args: FlashblocksArgs,
}

#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
#[command(next_help_heading = "Flashblocks")]
pub struct FlashblocksArgs {
	/// Flashblocks block-time target
	#[arg(
		long = "flashblocks.interval",
    name = "DURATION",
		default_value = "250ms",
		value_parser = humantime::parse_duration,
		env = "FLASHBLOCKS_INTERVAL"
	)]
	pub interval: Duration,

	/// Enables flashblocks publishing on the specified WebSocket address.
	/// If no address is specified defaults to 0.0.0.0:10111.
	#[arg(
		long = "flashblocks",
    name = "WS_ADDRESS",
		env = "FLASHBLOCKS_WS_ADDRESS",
    num_args = 0..=1,
		default_missing_value = "0.0.0.0:10111"
	)]
	enabled: Option<SocketAddr>,
}

impl Default for BuilderArgs {
	fn default() -> Self {
		let args = Cli::parse_from(["dummy", "node"]);
		let Commands::Node(node_command) = args.command else {
			unreachable!()
		};
		node_command.ext
	}
}

impl FlashblocksArgs {
	/// Returns true if flashblocks are enabled.
	pub fn enabled(&self) -> bool {
		self.enabled.is_some()
	}

	/// Returns the WebSocket address for flashblocks publishing socket.
	pub fn ws_address(&self) -> Option<SocketAddr> {
		self.enabled
	}
}

/// This type is used to store the builder's secret key for signing the last
/// transaction in the block.
#[derive(Debug, Clone, Deref, FromStr, Into, From)]
pub struct BuilderSigner {
	signer: PrivateKeySigner,
}

impl PartialEq for BuilderSigner {
	fn eq(&self, other: &Self) -> bool {
		self.signer.address() == other.signer.address()
	}
}
impl Eq for BuilderSigner {}

/// This trait is used to extend Reth's CLI with additional functionality that
/// are specific to the OP builder, such as populating default values for CLI
/// arguments when running in the playground mode or checking the builder mode.
pub trait CliExt {
	/// Populates the default values for the CLI arguments when the user specifies
	/// the `--builder.playground` flag.
	fn populate_defaults(self) -> Self;

	/// Returns the Cli instance with the parsed command line arguments
	/// and defaults populated if applicable.
	fn parsed() -> Self;

	/// Returns the Cli instance with the parsed command line arguments
	/// and replaces version, name, author, and about
	fn set_version() -> Self;
}

impl CliExt for Cli {
	fn parsed() -> Self {
		Cli::set_version().populate_defaults()
	}

	/// Checks if the node is started with the `--builder.playground` flag,
	/// and if so, populates the default values for the CLI arguments from the
	/// playground configuration.
	///
	/// The `--builder.playground` flag is used to populate the CLI arguments with
	/// default values for running the builder against the playground environment.
	///
	/// The values are populated from the default directory of the playground
	/// configuration, which is `$HOME/.playground/devnet/` by default.
	///
	/// Any manually specified CLI arguments by the user will override the
	/// defaults.
	fn populate_defaults(self) -> Self {
		let Commands::Node(ref node_command) = self.command else {
			// playground defaults are only relevant if running the node commands.
			return self;
		};

		let Some(ref playground_dir) = node_command.ext.playground else {
			// not running in playground mode.
			return self;
		};

		let options = match PlaygroundOptions::new(playground_dir) {
			Ok(options) => options,
			Err(e) => exit(&e),
		};

		options.apply(self)
	}

	fn set_version() -> Self {
		let matches = Cli::command()
			.version(env!("CARGO_PKG_VERSION"))
			.about("Flashbots block builder for Optimism")
			.author("Flashbots")
			.name("flashblocks")
			.get_matches();
		Cli::from_arg_matches(&matches).expect("Parsing args")
	}
}

/// Following clap's convention, a failure to parse the command line arguments
/// will result in terminating the program with a non-zero exit code.
fn exit(error: &eyre::Report) -> ! {
	eprintln!("{error}");
	std::process::exit(-1);
}

fn expand_path(s: &str) -> Result<PathBuf> {
	shellexpand::full(s)
		.map_err(|e| eyre!("expansion error for `{s}`: {e}"))?
		.into_owned()
		.parse()
		.map_err(|e| eyre!("invalid path after expansion: {e}"))
}

#[cfg(test)]
impl FlashblocksArgs {
	/// Configures flashblocks for tests. Handles WS port assignment.
	pub fn default_on_for_tests() -> Self {
		use {
			core::net::{Ipv4Addr, SocketAddrV4},
			std::{
				collections::HashSet,
				net::TcpListener,
				sync::{Mutex, OnceLock},
			},
		};

		static RESERVED_PORTS: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
		let reserved = RESERVED_PORTS.get_or_init(|| Mutex::new(HashSet::new()));

		let port = (12000..19000)
			.find(|port| {
				let addr = format!("0.0.0.0:{port}");
				if let Ok(listener) = TcpListener::bind(&addr) {
					drop(listener);
					let mut set = reserved.lock().unwrap();
					if set.contains(port) {
						false
					} else {
						set.insert(*port);
						true
					}
				} else {
					false
				}
			})
			.expect("No available ports found for test");

		Self {
			interval: Duration::from_millis(250),
			enabled: Some(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into()),
		}
	}
}
