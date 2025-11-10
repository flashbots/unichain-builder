//! Unit tests for CLI arguments, particularly the leeway-time argument

use {
	crate::args::Cli,
	clap::Parser,
	core::time::Duration,
	rblib::reth::optimism::cli::commands::Commands,
};

#[test]
fn test_leeway_time_cli_parsing() {
	// Test various duration formats
	let test_cases = vec![
		("100ms", Duration::from_millis(100)),
		("1s", Duration::from_secs(1)),
		("500ms", Duration::from_millis(500)),
		("2.5s", Duration::from_millis(2500)),
		("0ms", Duration::from_millis(0)),
	];

	for (input, expected) in test_cases {
		let args = Cli::try_parse_from([
			"flashblocks",
			"node",
			"--flashblocks.leeway-time",
			input,
		])
		.expect("Should parse successfully");

		if let Commands::Node(node_command) = args.command {
			assert_eq!(
				node_command.ext.flashblocks_args.leeway_time, expected,
				"Failed for input: {input}",
			);
		} else {
			panic!("Expected Node command");
		}
	}
}

#[test]
fn test_leeway_time_invalid_format() {
	let invalid_inputs = vec!["invalid", "100", "ms100", "-50ms"];

	for input in invalid_inputs {
		let result = Cli::try_parse_from([
			"flashblocks",
			"node",
			"--flashblocks.leeway-time",
			input,
		]);

		assert!(
			result.is_err(),
			"Should fail to parse invalid input: {input}",
		);
	}
}
