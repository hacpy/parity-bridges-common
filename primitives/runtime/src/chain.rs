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

use frame_support::Parameter;
use num_traits::{AsPrimitive, Bounded, CheckedSub, SaturatingAdd, Zero};
use sp_runtime::{
	traits::{
		AtLeast32Bit, AtLeast32BitUnsigned, Hash as HashT, Header as HeaderT, MaybeDisplay, MaybeMallocSizeOf,
		MaybeSerialize, MaybeSerializeDeserialize, Member, SimpleBitOps, Verify,
	},
	FixedPointOperand,
};
use sp_std::{convert::TryFrom, fmt::Debug, hash::Hash, str::FromStr};

/// Minimal Substrate-based chain representation that may be used from no_std environment.
pub trait Chain: Send + Sync + 'static {
	/// A type that fulfills the abstract idea of what a Substrate block number is.
	// Constraits come from the associated Number type of `sp_runtime::traits::Header`
	// See here for more info:
	// https://crates.parity.io/sp_runtime/traits/trait.Header.html#associatedtype.Number
	//
	// Note that the `AsPrimitive<usize>` trait is required by the GRANDPA justification
	// verifier, and is not usually part of a Substrate Header's Number type.
	type BlockNumber: Parameter
		+ Member
		+ MaybeSerializeDeserialize
		+ Hash
		+ Copy
		+ Default
		+ MaybeDisplay
		+ AtLeast32BitUnsigned
		+ FromStr
		+ MaybeMallocSizeOf
		+ AsPrimitive<usize>
		+ Default
		// original `sp_runtime::traits::Header::BlockNumber` doesn't have this trait, but
		// `sp_runtime::generic::Era` requires block number -> `u64` conversion.
		+ Into<u64>;

	/// A type that fulfills the abstract idea of what a Substrate hash is.
	// Constraits come from the associated Hash type of `sp_runtime::traits::Header`
	// See here for more info:
	// https://crates.parity.io/sp_runtime/traits/trait.Header.html#associatedtype.Hash
	type Hash: Parameter
		+ Member
		+ MaybeSerializeDeserialize
		+ Hash
		+ Ord
		+ Copy
		+ MaybeDisplay
		+ Default
		+ SimpleBitOps
		+ AsRef<[u8]>
		+ AsMut<[u8]>
		+ MaybeMallocSizeOf;

	/// A type that fulfills the abstract idea of what a Substrate hasher (a type
	/// that produces hashes) is.
	// Constraits come from the associated Hashing type of `sp_runtime::traits::Header`
	// See here for more info:
	// https://crates.parity.io/sp_runtime/traits/trait.Header.html#associatedtype.Hashing
	type Hasher: HashT<Output = Self::Hash>;

	/// A type that fulfills the abstract idea of what a Substrate header is.
	// See here for more info:
	// https://crates.parity.io/sp_runtime/traits/trait.Header.html
	type Header: Parameter + HeaderT<Number = Self::BlockNumber, Hash = Self::Hash> + MaybeSerializeDeserialize;

	/// The user account identifier type for the runtime.
	type AccountId: Parameter + Member + MaybeSerializeDeserialize + Debug + MaybeDisplay + Ord + Default;
	/// Balance of an account in native tokens.
	///
	/// The chain may support multiple tokens, but this particular type is for token that is used
	/// to pay for transaction dispatch, to reward different relayers (headers, messages), etc.
	type Balance: AtLeast32BitUnsigned
		+ FixedPointOperand
		+ Parameter
		+ Parameter
		+ Member
		+ MaybeSerializeDeserialize
		+ Clone
		+ Copy
		+ Bounded
		+ CheckedSub
		+ PartialOrd
		+ SaturatingAdd
		+ Zero
		+ TryFrom<sp_core::U256>;
	/// Index of a transaction used by the chain.
	type Index: Parameter
		+ Member
		+ MaybeSerialize
		+ Debug
		+ Default
		+ MaybeDisplay
		+ MaybeSerializeDeserialize
		+ AtLeast32Bit
		+ Copy;
	/// Signature type, used on this chain.
	type Signature: Parameter + Verify;
}

/// Block number used by the chain.
pub type BlockNumberOf<C> = <C as Chain>::BlockNumber;

/// Hash type used by the chain.
pub type HashOf<C> = <C as Chain>::Hash;

/// Hasher type used by the chain.
pub type HasherOf<C> = <C as Chain>::Hasher;

/// Header type used by the chain.
pub type HeaderOf<C> = <C as Chain>::Header;

/// Account id type used by the chain.
pub type AccountIdOf<C> = <C as Chain>::AccountId;

/// Balance type used by the chain.
pub type BalanceOf<C> = <C as Chain>::Balance;

/// Transaction index type used by the chain.
pub type IndexOf<C> = <C as Chain>::Index;

/// Signature type used by the chain.
pub type SignatureOf<C> = <C as Chain>::Signature;

/// Account public type used by the chain.
pub type AccountPublicOf<C> = <SignatureOf<C> as Verify>::Signer;

/// Transaction era used by the chain.
pub type TransactionEraOf<C> = crate::TransactionEra<BlockNumberOf<C>, HashOf<C>>;
