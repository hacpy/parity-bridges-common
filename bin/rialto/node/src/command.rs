// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use crate::cli::{Cli, Subcommand};
use crate::service::new_partial;
use rialto_runtime::{Block, RuntimeApi};
use sc_cli::{ChainSpec, Role, RuntimeVersion, SubstrateCli};
use sc_service::PartialComponents;

impl SubstrateCli for Cli {
	fn impl_name() -> String {
		"Rialto Bridge Node".into()
	}

	fn impl_version() -> String {
		env!("CARGO_PKG_VERSION").into()
	}

	fn description() -> String {
		"Rialto Bridge Node".into()
	}

	fn author() -> String {
		"Parity Technologies".into()
	}

	fn support_url() -> String {
		"https://github.com/paritytech/parity-bridges-common/".into()
	}

	fn copyright_start_year() -> i32 {
		2019
	}

	fn executable_name() -> String {
		"rialto-bridge-node".into()
	}

	fn native_runtime_version(_: &Box<dyn ChainSpec>) -> &'static RuntimeVersion {
		&rialto_runtime::VERSION
	}

	fn load_spec(&self, id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
		Ok(Box::new(
			match id {
				"" | "dev" => crate::chain_spec::Alternative::Development,
				"local" => crate::chain_spec::Alternative::LocalTestnet,
				_ => return Err(format!("Unsupported chain specification: {}", id)),
			}
			.load(),
		))
	}
}

/// Parse and run command line arguments
pub fn run() -> sc_cli::Result<()> {
	let cli = Cli::from_args();
	sp_core::crypto::set_default_ss58_version(sp_core::crypto::Ss58AddressFormat::Custom(
		rialto_runtime::SS58Prefix::get() as u16,
	));

	match &cli.subcommand {
		Some(Subcommand::Benchmark(cmd)) => {
			if cfg!(feature = "runtime-benchmarks") {
				let runner = cli.create_runner(cmd)?;

				runner.sync_run(|config| cmd.run::<Block, crate::service::Executor>(config))
			} else {
				println!(
					"Benchmarking wasn't enabled when building the node. \
				You can enable it with `--features runtime-benchmarks`."
				);
				Ok(())
			}
		}
		Some(Subcommand::Key(cmd)) => cmd.run(&cli),
		Some(Subcommand::Sign(cmd)) => cmd.run(),
		Some(Subcommand::Verify(cmd)) => cmd.run(),
		Some(Subcommand::Vanity(cmd)) => cmd.run(),
		Some(Subcommand::BuildSpec(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| cmd.run(config.chain_spec, config.network))
		}
		Some(Subcommand::CheckBlock(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let PartialComponents {
					client,
					task_manager,
					import_queue,
					..
				} = new_partial(&mut config).map_err(service_error)?;
				Ok((cmd.run(client, import_queue), task_manager))
			})
		}
		Some(Subcommand::ExportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let PartialComponents {
					client, task_manager, ..
				} = new_partial(&mut config).map_err(service_error)?;
				Ok((cmd.run(client, config.database), task_manager))
			})
		}
		Some(Subcommand::ExportState(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let PartialComponents {
					client, task_manager, ..
				} = new_partial(&mut config).map_err(service_error)?;
				Ok((cmd.run(client, config.chain_spec), task_manager))
			})
		}
		Some(Subcommand::ImportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let PartialComponents {
					client,
					task_manager,
					import_queue,
					..
				} = new_partial(&mut config).map_err(service_error)?;
				Ok((cmd.run(client, import_queue), task_manager))
			})
		}
		Some(Subcommand::PurgeChain(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| cmd.run(config.database))
		}
		Some(Subcommand::Revert(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let PartialComponents {
					client,
					task_manager,
					backend,
					..
				} = new_partial(&mut config).map_err(service_error)?;
				Ok((cmd.run(client, backend), task_manager))
			})
		}
		Some(Subcommand::Inspect(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| cmd.run::<Block, RuntimeApi, crate::service::Executor>(config))
		}
		Some(Subcommand::PvfPrepareWorker(cmd)) => {
			let mut builder = sc_cli::LoggerBuilder::new("");
			builder.with_colors(false);
			let _ = builder.init();

			polkadot_node_core_pvf::prepare_worker_entrypoint(&cmd.socket_path);
			Ok(())
		}
		Some(crate::cli::Subcommand::PvfExecuteWorker(cmd)) => {
			let mut builder = sc_cli::LoggerBuilder::new("");
			builder.with_colors(false);
			let _ = builder.init();

			polkadot_node_core_pvf::execute_worker_entrypoint(&cmd.socket_path);
			Ok(())
		}
		None => {
			let runner = cli.create_runner(&cli.run)?;

			// some parameters that are used by polkadot nodes, but that are not used by our binary
			// let jaeger_agent = None;
			// let grandpa_pause = None;
			// let no_beefy = true;
			// let telemetry_worker_handler = None;
			// let is_collator = crate::service::IsCollator::No;
			let overseer_gen = crate::overseer::RealOverseerGen;
			runner.run_node_until_exit(|config| async move {
				match config.role {
					Role::Light => Err(sc_cli::Error::Service(sc_service::Error::Other(
						"Light client is not supported by this node".into(),
					))),
					_ => crate::service::build_full(config, overseer_gen)
						.map(|full| full.task_manager)
						.map_err(service_error),
				}
			})
		}
	}
}

// We don't want to change 'service.rs' too much to ease future updates => it'll keep using
// its own error enum like original polkadot service does.
fn service_error(err: crate::service::Error) -> sc_cli::Error {
	sc_cli::Error::Application(Box::new(err))
}
