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

//! Primitives that may be used at (bridges) runtime level.

#![cfg_attr(not(feature = "std"), no_std)]

use codec::Encode;
use frame_support::RuntimeDebug;
use sp_core::hash::H256;
use sp_io::hashing::blake2_256;
use sp_std::convert::TryFrom;

pub use chain::{
	AccountIdOf, AccountPublicOf, BalanceOf, BlockNumberOf, Chain, HashOf, HasherOf, HeaderOf, IndexOf, SignatureOf,
	TransactionEraOf,
};
pub use storage_proof::{Error as StorageProofError, StorageProofChecker};

#[cfg(feature = "std")]
pub use storage_proof::craft_valid_storage_proof;

pub mod messages;

mod chain;
mod storage_proof;

/// Use this when something must be shared among all instances.
pub const NO_INSTANCE_ID: ChainId = [0, 0, 0, 0];

/// Bridge-with-Rialto instance id.
pub const RIALTO_CHAIN_ID: ChainId = *b"rlto";

/// Bridge-with-Millau instance id.
pub const MILLAU_CHAIN_ID: ChainId = *b"mlau";

/// Bridge-with-Polkadot instance id.
pub const POLKADOT_CHAIN_ID: ChainId = *b"pdot";

/// Bridge-with-Kusama instance id.
pub const KUSAMA_CHAIN_ID: ChainId = *b"ksma";

/// Bridge-with-Rococo instance id.
pub const ROCOCO_CHAIN_ID: ChainId = *b"roco";

/// Bridge-with-Wococo instance id.
pub const WOCOCO_CHAIN_ID: ChainId = *b"woco";

/// Call-dispatch module prefix.
pub const CALL_DISPATCH_MODULE_PREFIX: &[u8] = b"pallet-bridge/dispatch";

/// A unique prefix for entropy when generating cross-chain account IDs.
pub const ACCOUNT_DERIVATION_PREFIX: &[u8] = b"pallet-bridge/account-derivation/account";

/// A unique prefix for entropy when generating a cross-chain account ID for the Root account.
pub const ROOT_ACCOUNT_DERIVATION_PREFIX: &[u8] = b"pallet-bridge/account-derivation/root";

/// Unique identifier of the chain.
///
/// In addition to its main function (identifying the chain), this type may also be used to
/// identify module instance. We have a bunch of pallets that may be used in different bridges. E.g.
/// messages pallet may be deployed twice in the same runtime to bridge ThisChain with Chain1 and Chain2.
/// Sometimes we need to be able to identify deployed instance dynamically. This type may be used for that.
pub type ChainId = [u8; 4];

/// Type of accounts on the source chain.
pub enum SourceAccount<T> {
	/// An account that belongs to Root (privileged origin).
	Root,
	/// A non-privileged account.
	///
	/// The embedded account ID may or may not have a private key depending on the "owner" of the
	/// account (private key, pallet, proxy, etc.).
	Account(T),
}

/// Derive an account ID from a foreign account ID.
///
/// This function returns an encoded Blake2 hash. It is the responsibility of the caller to ensure
/// this can be successfully decoded into an AccountId.
///
/// The `bridge_id` is used to provide extra entropy when producing account IDs. This helps prevent
/// AccountId collisions between different bridges on a single target chain.
///
/// Note: If the same `bridge_id` is used across different chains (for example, if one source chain
/// is bridged to multiple target chains), then all the derived accounts would be the same across
/// the different chains. This could negatively impact users' privacy across chains.
pub fn derive_account_id<AccountId>(bridge_id: ChainId, id: SourceAccount<AccountId>) -> H256
where
	AccountId: Encode,
{
	match id {
		SourceAccount::Root => (ROOT_ACCOUNT_DERIVATION_PREFIX, bridge_id).using_encoded(blake2_256),
		SourceAccount::Account(id) => (ACCOUNT_DERIVATION_PREFIX, bridge_id, id).using_encoded(blake2_256),
	}
	.into()
}

/// Derive the account ID of the shared relayer fund account.
///
/// This account is used to collect fees for relayers that are passing messages across the bridge.
///
/// The account ID can be the same across different instances of `pallet-bridge-messages` if the same
/// `bridge_id` is used.
pub fn derive_relayer_fund_account_id(bridge_id: ChainId) -> H256 {
	("relayer-fund-account", bridge_id).using_encoded(blake2_256).into()
}

/// Anything that has size.
pub trait Size {
	/// Return approximate size of this object (in bytes).
	///
	/// This function should be lightweight. The result should not necessary be absolutely
	/// accurate.
	fn size_hint(&self) -> u32;
}

impl Size for () {
	fn size_hint(&self) -> u32 {
		0
	}
}

/// Pre-computed size.
pub struct PreComputedSize(pub usize);

impl Size for PreComputedSize {
	fn size_hint(&self) -> u32 {
		u32::try_from(self.0).unwrap_or(u32::MAX)
	}
}

/// Era of specific transaction.
#[derive(RuntimeDebug, Clone, Copy)]
pub enum TransactionEra<BlockNumber, BlockHash> {
	/// Transaction is immortal.
	Immortal,
	/// Transaction is valid for a given number of blocks, starting from given block.
	Mortal(BlockNumber, BlockHash, u32),
}

impl<BlockNumber: Copy + Into<u64>, BlockHash: Copy> TransactionEra<BlockNumber, BlockHash> {
	/// Prepare transaction era, based on mortality period and current best block number.
	pub fn new(best_block_number: BlockNumber, best_block_hash: BlockHash, mortality_period: Option<u32>) -> Self {
		mortality_period
			.map(|mortality_period| TransactionEra::Mortal(best_block_number, best_block_hash, mortality_period))
			.unwrap_or(TransactionEra::Immortal)
	}

	/// Create new immortal transaction era.
	pub fn immortal() -> Self {
		TransactionEra::Immortal
	}

	/// Returns era that is used by FRAME-based runtimes.
	pub fn frame_era(&self) -> sp_runtime::generic::Era {
		match *self {
			TransactionEra::Immortal => sp_runtime::generic::Era::immortal(),
			TransactionEra::Mortal(header_number, _, period) => {
				sp_runtime::generic::Era::mortal(period as _, header_number.into())
			}
		}
	}

	/// Returns header hash that needs to be included in the signature payload.
	pub fn signed_payload(&self, genesis_hash: BlockHash) -> BlockHash {
		match *self {
			TransactionEra::Immortal => genesis_hash,
			TransactionEra::Mortal(_, header_hash, _) => header_hash,
		}
	}
}
