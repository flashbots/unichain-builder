use {
	rblib::prelude::{Checkpoint, ControlFlow, Platform, Step, StepContext},
	std::sync::{
		Arc,
		atomic::{AtomicU64, Ordering},
	},
};

pub struct BreakAfterMaxFlashblocks {
	current_flashblock: Arc<AtomicU64>,
	max_flashblocks: Arc<AtomicU64>,
}

impl BreakAfterMaxFlashblocks {
	pub fn new(
		current_flashblock: Arc<AtomicU64>,
		max_flashblocks: Arc<AtomicU64>,
	) -> Self {
		Self {
			current_flashblock,
			max_flashblocks,
		}
	}
}

impl<P: Platform> Step<P> for BreakAfterMaxFlashblocks {
	async fn step(
		self: std::sync::Arc<Self>,
		payload: Checkpoint<P>,
		_: StepContext<P>,
	) -> ControlFlow<P> {
		if self.current_flashblock.load(Ordering::Relaxed)
			> self.max_flashblocks.load(Ordering::Relaxed)
		{
			ControlFlow::Break(payload)
		} else {
			ControlFlow::Ok(payload)
		}
	}
}
