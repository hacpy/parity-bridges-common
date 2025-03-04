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

//! Everything required to serve Millau <-> Rialto messages.

use crate::Runtime;

use bp_messages::{
	source_chain::TargetHeaderChain,
	target_chain::{ProvedMessages, SourceHeaderChain},
	InboundLaneData, LaneId, Message, MessageNonce, Parameter as MessagesParameter,
};
use bp_runtime::{ChainId, MILLAU_CHAIN_ID, RIALTO_CHAIN_ID};
use bridge_runtime_common::messages::{self, MessageBridge, MessageTransaction};
use codec::{Decode, Encode};
use frame_support::{
	parameter_types,
	weights::{DispatchClass, Weight},
	RuntimeDebug,
};
use sp_runtime::{traits::Saturating, FixedPointNumber, FixedU128};
use sp_std::{convert::TryFrom, ops::RangeInclusive};

/// Initial value of `MillauToRialtoConversionRate` parameter.
pub const INITIAL_MILLAU_TO_RIALTO_CONVERSION_RATE: FixedU128 = FixedU128::from_inner(FixedU128::DIV);
/// Initial value of `MillauFeeMultiplier` parameter.
pub const INITIAL_MILLAU_FEE_MULTIPLIER: FixedU128 = FixedU128::from_inner(FixedU128::DIV);

parameter_types! {
	/// Millau to Rialto conversion rate. Initially we treat both tokens as equal.
	pub storage MillauToRialtoConversionRate: FixedU128 = INITIAL_MILLAU_TO_RIALTO_CONVERSION_RATE;
	/// Fee multiplier value at Millau chain.
	pub storage MillauFeeMultiplier: FixedU128 = INITIAL_MILLAU_FEE_MULTIPLIER;
}

/// Message payload for Rialto -> Millau messages.
pub type ToMillauMessagePayload = messages::source::FromThisChainMessagePayload<WithMillauMessageBridge>;

/// Message verifier for Rialto -> Millau messages.
pub type ToMillauMessageVerifier = messages::source::FromThisChainMessageVerifier<WithMillauMessageBridge>;

/// Message payload for Millau -> Rialto messages.
pub type FromMillauMessagePayload = messages::target::FromBridgedChainMessagePayload<WithMillauMessageBridge>;

/// Encoded Rialto Call as it comes from Millau.
pub type FromMillauEncodedCall = messages::target::FromBridgedChainEncodedMessageCall<crate::Call>;

/// Call-dispatch based message dispatch for Millau -> Rialto messages.
pub type FromMillauMessageDispatch = messages::target::FromBridgedChainMessageDispatch<
	WithMillauMessageBridge,
	crate::Runtime,
	pallet_balances::Pallet<Runtime>,
	(),
>;

/// Messages proof for Millau -> Rialto messages.
pub type FromMillauMessagesProof = messages::target::FromBridgedChainMessagesProof<bp_millau::Hash>;

/// Messages delivery proof for Rialto -> Millau messages.
pub type ToMillauMessagesDeliveryProof = messages::source::FromBridgedChainMessagesDeliveryProof<bp_millau::Hash>;

/// Millau <-> Rialto message bridge.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct WithMillauMessageBridge;

impl MessageBridge for WithMillauMessageBridge {
	const RELAYER_FEE_PERCENT: u32 = 10;
	const THIS_CHAIN_ID: ChainId = RIALTO_CHAIN_ID;
	const BRIDGED_CHAIN_ID: ChainId = MILLAU_CHAIN_ID;
	const BRIDGED_MESSAGES_PALLET_NAME: &'static str = bp_millau::WITH_RIALTO_MESSAGES_PALLET_NAME;

	type ThisChain = Rialto;
	type BridgedChain = Millau;

	fn bridged_balance_to_this_balance(bridged_balance: bp_millau::Balance) -> bp_rialto::Balance {
		bp_rialto::Balance::try_from(MillauToRialtoConversionRate::get().saturating_mul_int(bridged_balance))
			.unwrap_or(bp_rialto::Balance::MAX)
	}
}

/// Rialto chain from message lane point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Rialto;

impl messages::ChainWithMessages for Rialto {
	type Hash = bp_rialto::Hash;
	type AccountId = bp_rialto::AccountId;
	type Signer = bp_rialto::AccountSigner;
	type Signature = bp_rialto::Signature;
	type Weight = Weight;
	type Balance = bp_rialto::Balance;
}

impl messages::ThisChainWithMessages for Rialto {
	type Call = crate::Call;

	fn is_outbound_lane_enabled(lane: &LaneId) -> bool {
		*lane == [0, 0, 0, 0] || *lane == [0, 0, 0, 1]
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		MessageNonce::MAX
	}

	fn estimate_delivery_confirmation_transaction() -> MessageTransaction<Weight> {
		let inbound_data_size = InboundLaneData::<bp_rialto::AccountId>::encoded_size_hint(
			bp_rialto::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
			1,
			1,
		)
		.unwrap_or(u32::MAX);

		MessageTransaction {
			dispatch_weight: bp_rialto::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			size: inbound_data_size
				.saturating_add(bp_millau::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_rialto::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_rialto::Balance {
		// `transaction` may represent transaction from the future, when multiplier value will
		// be larger, so let's use slightly increased value
		let multiplier = FixedU128::saturating_from_rational(110, 100)
			.saturating_mul(pallet_transaction_payment::Pallet::<Runtime>::next_fee_multiplier());
		// in our testnets, both per-byte fee and weight-to-fee are 1:1
		messages::transaction_payment(
			bp_rialto::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			1,
			multiplier,
			|weight| weight as _,
			transaction,
		)
	}
}

/// Millau chain from message lane point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Millau;

impl messages::ChainWithMessages for Millau {
	type Hash = bp_millau::Hash;
	type AccountId = bp_millau::AccountId;
	type Signer = bp_millau::AccountSigner;
	type Signature = bp_millau::Signature;
	type Weight = Weight;
	type Balance = bp_millau::Balance;
}

impl messages::BridgedChainWithMessages for Millau {
	fn maximal_extrinsic_size() -> u32 {
		bp_millau::max_extrinsic_size()
	}

	fn message_weight_limits(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit = messages::target::maximal_incoming_message_dispatch_weight(bp_millau::max_extrinsic_weight());

		// we're charging for payload bytes in `WithMillauMessageBridge::transaction_payment` function
		//
		// this bridge may be used to deliver all kind of messages, so we're not making any assumptions about
		// minimal dispatch weight here

		0..=upper_limit
	}

	fn estimate_delivery_transaction(
		message_payload: &[u8],
		include_pay_dispatch_fee_cost: bool,
		message_dispatch_weight: Weight,
	) -> MessageTransaction<Weight> {
		let message_payload_len = u32::try_from(message_payload.len()).unwrap_or(u32::MAX);
		let extra_bytes_in_payload = Weight::from(message_payload_len)
			.saturating_sub(pallet_bridge_messages::EXPECTED_DEFAULT_MESSAGE_LENGTH.into());

		MessageTransaction {
			dispatch_weight: extra_bytes_in_payload
				.saturating_mul(bp_millau::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT)
				.saturating_add(bp_millau::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT)
				.saturating_sub(if include_pay_dispatch_fee_cost {
					0
				} else {
					bp_millau::PAY_INBOUND_DISPATCH_FEE_WEIGHT
				})
				.saturating_add(message_dispatch_weight),
			size: message_payload_len
				.saturating_add(bp_rialto::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_millau::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_millau::Balance {
		// we don't have a direct access to the value of multiplier at Millau chain
		// => it is a messages module parameter
		let multiplier = MillauFeeMultiplier::get();
		// in our testnets, both per-byte fee and weight-to-fee are 1:1
		messages::transaction_payment(
			bp_millau::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			1,
			multiplier,
			|weight| weight as _,
			transaction,
		)
	}
}

impl TargetHeaderChain<ToMillauMessagePayload, bp_millau::AccountId> for Millau {
	type Error = &'static str;
	// The proof is:
	// - hash of the header this proof has been created with;
	// - the storage proof of one or several keys;
	// - id of the lane we prove state of.
	type MessagesDeliveryProof = ToMillauMessagesDeliveryProof;

	fn verify_message(payload: &ToMillauMessagePayload) -> Result<(), Self::Error> {
		messages::source::verify_chain_message::<WithMillauMessageBridge>(payload)
	}

	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<bp_rialto::AccountId>), Self::Error> {
		messages::source::verify_messages_delivery_proof::<WithMillauMessageBridge, Runtime, crate::MillauGrandpaInstance>(
			proof,
		)
	}
}

impl SourceHeaderChain<bp_millau::Balance> for Millau {
	type Error = &'static str;
	// The proof is:
	// - hash of the header this proof has been created with;
	// - the storage proof of one or several keys;
	// - id of the lane we prove messages for;
	// - inclusive range of messages nonces that are proved.
	type MessagesProof = FromMillauMessagesProof;

	fn verify_messages_proof(
		proof: Self::MessagesProof,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<bp_millau::Balance>>, Self::Error> {
		messages::target::verify_messages_proof::<WithMillauMessageBridge, Runtime, crate::MillauGrandpaInstance>(
			proof,
			messages_count,
		)
	}
}

/// Rialto -> Millau message lane pallet parameters.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq)]
pub enum RialtoToMillauMessagesParameter {
	/// The conversion formula we use is: `RialtoTokens = MillauTokens * conversion_rate`.
	MillauToRialtoConversionRate(FixedU128),
}

impl MessagesParameter for RialtoToMillauMessagesParameter {
	fn save(&self) {
		match *self {
			RialtoToMillauMessagesParameter::MillauToRialtoConversionRate(ref conversion_rate) => {
				MillauToRialtoConversionRate::set(conversion_rate)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{AccountId, Call, ExistentialDeposit, Runtime, SystemCall, SystemConfig, VERSION};
	use bp_message_dispatch::CallOrigin;
	use bp_messages::{
		target_chain::{DispatchMessage, DispatchMessageData, MessageDispatch},
		MessageKey,
	};
	use bp_runtime::{derive_account_id, messages::DispatchFeePayment, SourceAccount};
	use bridge_runtime_common::messages::target::{FromBridgedChainEncodedMessageCall, FromBridgedChainMessagePayload};
	use frame_support::{
		traits::Currency,
		weights::{GetDispatchInfo, WeightToFeePolynomial},
	};
	use sp_runtime::traits::Convert;

	#[test]
	fn transfer_happens_when_dispatch_fee_is_paid_at_target_chain() {
		// this test actually belongs to the `bridge-runtime-common` crate, but there we have no
		// mock runtime. Making another one there just for this test, given that both crates
		// live n single repo is an overkill
		let mut ext: sp_io::TestExternalities = SystemConfig::default().build_storage::<Runtime>().unwrap().into();
		ext.execute_with(|| {
			let bridge = MILLAU_CHAIN_ID;
			let call: Call = SystemCall::remark(vec![]).into();
			let dispatch_weight = call.get_dispatch_info().weight;
			let dispatch_fee = <Runtime as pallet_transaction_payment::Config>::WeightToFee::calc(&dispatch_weight);
			assert!(dispatch_fee > 0);

			// create relayer account with minimal balance
			let relayer_account: AccountId = [1u8; 32].into();
			let initial_amount = ExistentialDeposit::get();
			let _ = <pallet_balances::Pallet<Runtime> as Currency<AccountId>>::deposit_creating(
				&relayer_account,
				initial_amount,
			);

			// create dispatch account with minimal balance + dispatch fee
			let dispatch_account = derive_account_id::<<Runtime as pallet_bridge_dispatch::Config>::SourceChainAccountId>(
				bridge,
				SourceAccount::Root,
			);
			let dispatch_account =
				<Runtime as pallet_bridge_dispatch::Config>::AccountIdConverter::convert(dispatch_account);
			let _ = <pallet_balances::Pallet<Runtime> as Currency<AccountId>>::deposit_creating(
				&dispatch_account,
				initial_amount + dispatch_fee,
			);

			// dispatch message with intention to pay dispatch fee at the target chain
			FromMillauMessageDispatch::dispatch(
				&relayer_account,
				DispatchMessage {
					key: MessageKey {
						lane_id: Default::default(),
						nonce: 0,
					},
					data: DispatchMessageData {
						payload: Ok(FromBridgedChainMessagePayload::<WithMillauMessageBridge> {
							spec_version: VERSION.spec_version,
							weight: dispatch_weight,
							origin: CallOrigin::SourceRoot,
							dispatch_fee_payment: DispatchFeePayment::AtTargetChain,
							call: FromBridgedChainEncodedMessageCall::new(call.encode()),
						}),
						fee: 1,
					},
				},
			);

			// ensure that fee has been transferred from dispatch to relayer account
			assert_eq!(
				<pallet_balances::Pallet<Runtime> as Currency<AccountId>>::free_balance(&relayer_account),
				initial_amount + dispatch_fee,
			);
			assert_eq!(
				<pallet_balances::Pallet<Runtime> as Currency<AccountId>>::free_balance(&dispatch_account),
				initial_amount,
			);
		});
	}
}
