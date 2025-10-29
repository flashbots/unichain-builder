use {
	crate::state::FlashblockNumber,
	rblib::prelude::{Checkpoint, ControlFlow, Platform, Step, StepContext},
	std::sync::Arc,
};

pub struct BreakAfterMaxFlashblocks {
	flashblock_number: Arc<FlashblockNumber>,
}

impl BreakAfterMaxFlashblocks {
	pub fn new(flashblock_number: Arc<FlashblockNumber>) -> Self {
		Self { flashblock_number }
	}
}

impl<P: Platform> Step<P> for BreakAfterMaxFlashblocks {
	async fn step(
		self: std::sync::Arc<Self>,
		payload: Checkpoint<P>,
		_: StepContext<P>,
	) -> ControlFlow<P> {
		if !self.flashblock_number.in_bounds() {
			ControlFlow::Break(payload)
		} else {
			ControlFlow::Ok(payload)
		}
	}
}
