use {
	crate::{Flashblocks, state::TargetFlashblocks},
	rblib::prelude::{Checkpoint, ControlFlow, Step, StepContext},
	std::sync::Arc,
};

pub struct BreakAfterMaxFlashblocks {
	target_flashblocks: Arc<TargetFlashblocks>,
}

impl BreakAfterMaxFlashblocks {
	pub fn new(target_flashblocks: Arc<TargetFlashblocks>) -> Self {
		Self { target_flashblocks }
	}
}

impl Step<Flashblocks> for BreakAfterMaxFlashblocks {
	async fn step(
		self: std::sync::Arc<Self>,
		payload: Checkpoint<Flashblocks>,
		_: StepContext<Flashblocks>,
	) -> ControlFlow<Flashblocks> {
		if payload.context().current() <= self.target_flashblocks.get() {
			ControlFlow::Ok(payload)
		} else {
			ControlFlow::Break(payload)
		}
	}
}
