use rblib::reth::metrics::metrics::gauge;

pub fn set_version_metric() {
	let labels = [
		("cargo_pkg_name", env!("CARGO_PKG_NAME")),
		("cargo_pkg_version", env!("CARGO_PKG_VERSION")),
		("cargo_features", env!("VERGEN_CARGO_FEATURES")),
		("cargo_target_triple", env!("VERGEN_CARGO_TARGET_TRIPLE")),
		("build_timestamp", env!("VERGEN_BUILD_TIMESTAMP")),
		("build_profile", env!("BUILD_PROFILE")),
		("git_sha_short", env!("VERGEN_GIT_SHA_SHORT")),
		("git_tag", env!("VERGEN_GIT_DESCRIBE")),
		("git_commit_author", env!("VERGEN_GIT_COMMIT_AUTHOR")),
		("git_commit_message", env!("VERGEN_GIT_COMMIT_MESSAGE")),
		("short_version", env!("SHORT_VERSION")),
		("long_version", env!("LONG_VERSION")),
	];

	let gauge = gauge!("builder_version", &labels);
	gauge.set(1);
}
