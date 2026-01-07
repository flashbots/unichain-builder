use {
	std::{env, error::Error},
	vergen::{BuildBuilder, CargoBuilder, Emitter},
	vergen_git2::Git2Builder,
};

fn set_vergen_env_vars() -> Result<(), Box<dyn Error>> {
	let build = BuildBuilder::default().build_timestamp(true).build()?;
	let cargo = CargoBuilder::default()
		.features(true)
		.target_triple(true)
		.build()?;
	let git = Git2Builder::default()
		.describe(false, true, None)
		.dirty(true)
		.sha(false)
		.commit_author_name(true)
		.commit_author_email(true)
		.commit_message(true)
		.build()?;

	Emitter::default()
		.add_instructions(&build)?
		.add_instructions(&cargo)?
		.add_instructions(&git)?
		.emit_and_set()?;

	Ok(())
}

fn set_custom_env_vars() -> Result<(), Box<dyn Error>> {
	let git_sha = env::var("VERGEN_GIT_SHA")?;
	let git_sha_short = &git_sha[0..7];

	let git_author_name = env::var("VERGEN_GIT_COMMIT_AUTHOR_NAME")?;
	let git_author_email = env::var("VERGEN_GIT_COMMIT_AUTHOR_EMAIL")?;
	let git_author_full = format!("{git_author_name} <{git_author_email}>");

	let is_git_dirty = env::var("VERGEN_GIT_DIRTY")? == "true";
	// > git describe --always --tags
	// if not on a tag: v0.2.0-beta.3-82-g1939939b
	// if on a tag: v0.2.0-beta.3
	let not_on_git_tag =
		env::var("VERGEN_GIT_DESCRIBE")?.ends_with(&format!("-g{git_sha_short}"));
	let version_suffix = if is_git_dirty || not_on_git_tag {
		"-dev"
	} else {
		""
	};

	// Set the build profile
	let out_dir = env::var("OUT_DIR").unwrap();
	let build_profile = out_dir.rsplit(std::path::MAIN_SEPARATOR).nth(3).unwrap();

	// Set formatted version strings
	let cargo_pkg_name = env!("CARGO_PKG_NAME");
	let cargo_pkg_version = env!("CARGO_PKG_VERSION");

	// The short version information for op-rbuilder.
	// - The latest version from Cargo.toml
	// - The short SHA of the latest commit.
	// Example: 0.1.0 (defa64b2)
	let short_version =
		format!("{cargo_pkg_version}{version_suffix} ({git_sha_short})");

	// LONG_VERSION
	//
	// - The latest version from Cargo.toml + version suffix (if any)
	// - The full SHA of the latest commit
	// - The build datetime
	// - The cargo features
	// - The build profile
	// - The latest commit message and author
	//
	// Example:
	//
	// ```text
	// Version: 0.1.0
	// Commit SHA: defa64b2
	// Build Timestamp: 2023-05-19T01:47:19.815651705Z
	// Cargo Features: jemalloc
	// Build Profile: maxperf
	// Latest Commit: 'message' by John Doe <john.doe@example.com>
	// ```
	let long_version = format!(
		"Version: {cargo_pkg_version}{version_suffix}
		Commit SHA: {git_sha}
		Build Timestamp: {build_timestamp}
		Cargo Features: {cargo_features}
		Build Profile: {build_profile}
		Latest Commit: '{git_commit_message}' by {git_author_full}",
		cargo_pkg_version = cargo_pkg_version,
		version_suffix = version_suffix,
		git_sha = git_sha,
		build_timestamp = env::var("VERGEN_BUILD_TIMESTAMP")?,
		cargo_features = env::var("VERGEN_CARGO_FEATURES")?,
		build_profile = build_profile,
		git_commit_message = env::var("VERGEN_GIT_COMMIT_MESSAGE")?.trim_end(),
		git_author_full = git_author_full,
	);

	// The version information for op-rbuilder formatted for P2P (devp2p).
	// - The latest version from Cargo.toml
	// - The target triple
	//
	// Example: unichain-builder/v0.1.0-alpha.1-428a6dc2f/aarch64-apple-darwin
	let p2p_client_version = format_args!(
		"{cargo_pkg_name}/v{cargo_pkg_version}-{git_sha_short}/{}",
		env::var("VERGEN_CARGO_TARGET_TRIPLE")?
	)
	.to_string();

	let custom_env_vars = [
		("VERGEN_GIT_SHA_SHORT", &git_sha[..8]),
		("VERGEN_GIT_COMMIT_AUTHOR", &git_author_full),
		("VERSION_SUFFIX", version_suffix),
		("BUILD_PROFILE", build_profile),
		("SHORT_VERSION", &short_version),
		("LONG_VERSION", &long_version),
		("P2P_CLIENT_VERSION", &p2p_client_version),
	];

	for (name, value) in custom_env_vars {
		println!("cargo:rustc-env={name}={value}");
	}

	Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
	set_vergen_env_vars()?;
	set_custom_env_vars()?;

	Ok(())
}
