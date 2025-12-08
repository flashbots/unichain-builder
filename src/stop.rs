use {crate::Flashblocks, rblib::prelude::*, std::time::Duration};

pub struct BreakAfterMaxFlashblocks {
	flashblock_interval: Duration,
}

impl BreakAfterMaxFlashblocks {
	pub fn new(flashblock_interval: Duration) -> Self {
		Self {
			flashblock_interval,
		}
	}
}

impl Step<Flashblocks> for BreakAfterMaxFlashblocks {
	async fn step(
		self: std::sync::Arc<Self>,
		payload: Checkpoint<Flashblocks>,
		ctx: StepContext<Flashblocks>,
	) -> ControlFlow<Flashblocks> {
		let payload_deadline = ctx.limits().deadline.expect(
			"Flashblock limit require its enclosing scope to have a deadline",
		);

		let payload_deadline_ms = u64::try_from(payload_deadline.as_millis())
			.expect("payload_deadline should never exceed u64::MAX milliseconds");
		let flashblock_interval_ms = u64::try_from(
			self.flashblock_interval.as_millis(),
		)
		.expect("flashblock_interval should never exceed u64::MAX milliseconds");

		let offset_ms = payload_deadline_ms % flashblock_interval_ms;

		let target_flashblocks = if offset_ms == 0 {
			// Perfect division: payload time is exact multiple of interval
			payload_deadline_ms / flashblock_interval_ms
		} else {
			// Non-perfect division: add extra flashblock with shortened first
			// interval
			payload_deadline_ms / flashblock_interval_ms + 1
		};

		if payload.context().current() <= target_flashblocks {
			ControlFlow::Ok(payload)
		} else {
			ControlFlow::Break(payload)
		}
	}
}
