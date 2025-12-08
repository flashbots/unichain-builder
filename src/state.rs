use {
	crate::Flashblocks,
	rblib::prelude::CheckpointContext,
	std::fmt::Display,
};

/// Current flashblock number (1-indexed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashblockNumber(u64);

impl FlashblockNumber {
	pub fn new() -> Self {
		Self(1)
	}

	/// Returns current flashblock in 0-index format
	pub fn index(&self) -> u64 {
		self.0 - 1
	}

	pub fn current(&self) -> u64 {
		self.0
	}

	#[must_use]
	pub fn next(&self) -> Self {
		Self(self.0 + 1)
	}
}

impl Default for FlashblockNumber {
	fn default() -> Self {
		Self::new()
	}
}

impl Display for FlashblockNumber {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.current())
	}
}

impl CheckpointContext<Flashblocks> for FlashblockNumber {}
