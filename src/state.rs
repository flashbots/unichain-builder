use std::{
	fmt::Display,
	sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug)]
pub struct FlashblockNumber {
	/// Current flashblock number (1-indexed).
	current_flashblock: AtomicU64,
	/// Number of flashblocks we're targeting to build for this block.
	target_flashblocks: AtomicU64,
}

impl FlashblockNumber {
	pub fn new() -> Self {
		Self {
			current_flashblock: AtomicU64::new(1),
			target_flashblocks: AtomicU64::new(0),
		}
	}

	pub fn current(&self) -> u64 {
		self.current_flashblock.load(Ordering::Relaxed)
	}

	pub fn max(&self) -> u64 {
		self.target_flashblocks.load(Ordering::Relaxed)
	}

	pub fn advance(&self) -> u64 {
		self.current_flashblock.fetch_add(1, Ordering::Relaxed)
	}

	pub fn in_bounds(&self) -> bool {
		self.current_flashblock.load(Ordering::Relaxed)
			<= self.target_flashblocks.load(Ordering::Relaxed)
	}

	pub fn set_target_flashblocks(&self, target_flashblock: u64) {
		self
			.target_flashblocks
			.store(target_flashblock, Ordering::Relaxed)
	}

	pub fn reset_current_flashblock(&self) -> u64 {
		self.current_flashblock.swap(1, Ordering::Relaxed)
	}
}

impl Default for FlashblockNumber {
	fn default() -> Self {
		Self::new()
	}
}

impl Display for FlashblockNumber {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}/{}", self.current(), self.max())
	}
}
