use {
	crate::Cli,
	chrono::Local,
	clap::Args,
	colored::Colorize,
	futures::StreamExt,
	human_format::Formatter,
	rollup_boost_types::flashblocks::FlashblocksPayloadV1,
	std::time::Instant,
	tokio_tungstenite::{connect_async, tungstenite::Message},
};

#[derive(Debug, Clone, Args)]
pub struct WatchArgs {}

pub async fn run(cli: &Cli, _: &WatchArgs) -> eyre::Result<()> {
	let (mut stream, _) = connect_async(cli.ws.as_str()).await?;
	println!("Connected to {}", cli.ws);

	let mut prev_at: Option<Instant> = None;
	let mut prev: Option<FlashblocksPayloadV1> = None;

	while let Some(msg) = stream.next().await {
		let now = Instant::now();

		if let Err(e) = msg {
			eyre::bail!(e);
		}

		let Ok(Message::Text(msg)) = msg else {
			eyre::bail!("Received non-text message: {msg:?}");
		};

		let block: FlashblocksPayloadV1 = serde_json::from_str(&msg)?;

		if block.index == 0 {
			println!(); // start of new block
			println!();
		}

		let Some(ref base) = block.base else {
			println!("⚡️ #{} (no base), block: {block:#?}", block.index);
			continue;
		};

		if block.index == 0 {
			println!();
			let time = Local::now().time().format("%H:%M:%S%.3f").to_string();
			println!("🔗 Block #{} {}", base.block_number, time.dimmed());
		}

		let time_diff = match prev_at {
			Some(prev) => format!(
				" +{} ms",
				(Instant::now().saturating_duration_since(prev)).as_millis()
			),
			None => String::new(),
		};

		let gas_diff = match prev {
			Some(ref prev) => {
				let this_gas = block.diff.gas_used.saturating_sub(prev.diff.gas_used);
				format!(
					" +{} ({}%) gas",
					format_gas(this_gas),
					format_percent(this_gas, base.gas_limit)
				)
			}
			None => String::new(),
		};

		println!(
			" - ⚡️ {}, gas: {}/{} ({}%), {} txs {} {}",
			block.index,
			format_gas(block.diff.gas_used),
			format_gas(base.gas_limit),
			format_percent(block.diff.gas_used, base.gas_limit),
			block.diff.transactions.len(),
			time_diff.dimmed(),
			gas_diff.dimmed()
		);

		prev = Some(block);
		prev_at = Some(now);
	}

	Ok(())
}

fn format_gas(value: u64) -> String {
	Formatter::new()
		.with_decimals(1)
		.with_separator("")
		.format(value as f64)
}

fn format_percent(num: u64, dem: u64) -> String {
	Formatter::new()
		.with_decimals(1)
		.with_separator("")
		.format((num as f64 / dem as f64) * 100.0)
}
