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

//! Types that allow runtime to act as a source/target endpoint of message lanes.
//!
//! Messages are assumed to be encoded `Call`s of the target chain. Call-dispatch
//! pallet is used to dispatch incoming messages. Message identified by a tuple
//! of to elements - message lane id and message nonce.

use bp_message_dispatch::MessageDispatch as _;
use bp_messages::{
	source_chain::{LaneMessageVerifier, Sender},
	target_chain::{DispatchMessage, MessageDispatch, ProvedLaneMessages, ProvedMessages},
	InboundLaneData, LaneId, Message, MessageData, MessageKey, MessageNonce, OutboundLaneData,
};
use bp_runtime::{
	messages::{DispatchFeePayment, MessageDispatchResult},
	ChainId, Size, StorageProofChecker,
};
use codec::{Decode, Encode};
use frame_support::{
	traits::{Currency, ExistenceRequirement},
	weights::{Weight, WeightToFeePolynomial},
	RuntimeDebug,
};
use hash_db::Hasher;
use sp_runtime::{
	traits::{AtLeast32BitUnsigned, CheckedAdd, CheckedDiv, CheckedMul, Saturating, Zero},
	FixedPointNumber, FixedPointOperand, FixedU128,
};
use sp_std::{cmp::PartialOrd, convert::TryFrom, fmt::Debug, marker::PhantomData, ops::RangeInclusive, vec::Vec};
use sp_trie::StorageProof;

/// Bidirectional message bridge.
pub trait MessageBridge {
	/// Relayer interest (in percents).
	const RELAYER_FEE_PERCENT: u32;

	/// Identifier of this chain.
	const THIS_CHAIN_ID: ChainId;
	/// Identifier of the Bridged chain.
	const BRIDGED_CHAIN_ID: ChainId;
	/// Name of the paired messages pallet instance at the Bridged chain.
	///
	/// Should be the name that is used in the `construct_runtime!()` macro.
	const BRIDGED_MESSAGES_PALLET_NAME: &'static str;

	/// This chain in context of message bridge.
	type ThisChain: ThisChainWithMessages;
	/// Bridged chain in context of message bridge.
	type BridgedChain: BridgedChainWithMessages;

	/// Convert Bridged chain balance into This chain balance.
	fn bridged_balance_to_this_balance(bridged_balance: BalanceOf<BridgedChain<Self>>) -> BalanceOf<ThisChain<Self>>;
}

/// Chain that has `pallet-bridge-messages` and `dispatch` modules.
pub trait ChainWithMessages {
	/// Hash used in the chain.
	type Hash: Decode;
	/// Accound id on the chain.
	type AccountId: Encode + Decode;
	/// Public key of the chain account that may be used to verify signatures.
	type Signer: Encode + Decode;
	/// Signature type used on the chain.
	type Signature: Encode + Decode;
	/// Type of weight that is used on the chain. This would almost always be a regular
	/// `frame_support::weight::Weight`. But since the meaning of weight on different chains
	/// may be different, the `WeightOf<>` construct is used to avoid confusion between
	/// different weights.
	type Weight: From<frame_support::weights::Weight> + PartialOrd;
	/// Type of balances that is used on the chain.
	type Balance: Encode + Decode + CheckedAdd + CheckedDiv + CheckedMul + PartialOrd + From<u32> + Copy;
}

/// Message related transaction parameters estimation.
#[derive(RuntimeDebug)]
pub struct MessageTransaction<Weight> {
	/// The estimated dispatch weight of the transaction.
	pub dispatch_weight: Weight,
	/// The estimated size of the encoded transaction.
	pub size: u32,
}

/// This chain that has `pallet-bridge-messages` and `dispatch` modules.
pub trait ThisChainWithMessages: ChainWithMessages {
	/// Call type on the chain.
	type Call: Encode + Decode;

	/// Are we accepting any messages to the given lane?
	fn is_outbound_lane_enabled(lane: &LaneId) -> bool;

	/// Maximal number of pending (not yet delivered) messages at This chain.
	///
	/// Any messages over this limit, will be rejected.
	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce;

	/// Estimate size and weight of single message delivery confirmation transaction at This chain.
	fn estimate_delivery_confirmation_transaction() -> MessageTransaction<WeightOf<Self>>;

	/// Returns minimal transaction fee that must be paid for given transaction at This chain.
	fn transaction_payment(transaction: MessageTransaction<WeightOf<Self>>) -> BalanceOf<Self>;
}

/// Bridged chain that has `pallet-bridge-messages` and `dispatch` modules.
pub trait BridgedChainWithMessages: ChainWithMessages {
	/// Maximal extrinsic size at Bridged chain.
	fn maximal_extrinsic_size() -> u32;

	/// Returns feasible weights range for given message payload at the Bridged chain.
	///
	/// If message is being sent with the weight that is out of this range, then it
	/// should be rejected.
	///
	/// Weights returned from this function shall not include transaction overhead
	/// (like weight of signature and signed extensions verification), because they're
	/// already accounted by the `weight_of_delivery_transaction`. So this function should
	/// return pure call dispatch weights range.
	fn message_weight_limits(message_payload: &[u8]) -> RangeInclusive<Self::Weight>;

	/// Estimate size and weight of single message delivery transaction at the Bridged chain.
	fn estimate_delivery_transaction(
		message_payload: &[u8],
		include_pay_dispatch_fee_cost: bool,
		message_dispatch_weight: WeightOf<Self>,
	) -> MessageTransaction<WeightOf<Self>>;

	/// Returns minimal transaction fee that must be paid for given transaction at the Bridged chain.
	fn transaction_payment(transaction: MessageTransaction<WeightOf<Self>>) -> BalanceOf<Self>;
}

pub(crate) type ThisChain<B> = <B as MessageBridge>::ThisChain;
pub(crate) type BridgedChain<B> = <B as MessageBridge>::BridgedChain;
pub(crate) type HashOf<C> = <C as ChainWithMessages>::Hash;
pub(crate) type AccountIdOf<C> = <C as ChainWithMessages>::AccountId;
pub(crate) type SignerOf<C> = <C as ChainWithMessages>::Signer;
pub(crate) type SignatureOf<C> = <C as ChainWithMessages>::Signature;
pub(crate) type WeightOf<C> = <C as ChainWithMessages>::Weight;
pub(crate) type BalanceOf<C> = <C as ChainWithMessages>::Balance;

pub(crate) type CallOf<C> = <C as ThisChainWithMessages>::Call;

/// Raw storage proof type (just raw trie nodes).
type RawStorageProof = Vec<Vec<u8>>;

/// Compute fee of transaction at runtime where regular transaction payment pallet is being used.
///
/// The value of `multiplier` parameter is the expected value of `pallet_transaction_payment::NextFeeMultiplier`
/// at the moment when transaction is submitted. If you're charging this payment in advance (and that's what
/// happens with delivery and confirmation transaction in this crate), then there's a chance that the actual
/// fee will be larger than what is paid in advance. So the value must be chosen carefully.
pub fn transaction_payment<Balance: AtLeast32BitUnsigned + FixedPointOperand>(
	base_extrinsic_weight: Weight,
	per_byte_fee: Balance,
	multiplier: FixedU128,
	weight_to_fee: impl Fn(Weight) -> Balance,
	transaction: MessageTransaction<Weight>,
) -> Balance {
	// base fee is charged for every tx
	let base_fee = weight_to_fee(base_extrinsic_weight);

	// non-adjustable per-byte fee
	let len_fee = per_byte_fee.saturating_mul(Balance::from(transaction.size));

	// the adjustable part of the fee
	let unadjusted_weight_fee = weight_to_fee(transaction.dispatch_weight);
	let adjusted_weight_fee = multiplier.saturating_mul_int(unadjusted_weight_fee);

	base_fee.saturating_add(len_fee).saturating_add(adjusted_weight_fee)
}

/// Sub-module that is declaring types required for processing This -> Bridged chain messages.
pub mod source {
	use super::*;

	/// Encoded Call of the Bridged chain. We never try to decode it on This chain.
	pub type BridgedChainOpaqueCall = Vec<u8>;

	/// Message payload for This -> Bridged chain messages.
	pub type FromThisChainMessagePayload<B> = bp_message_dispatch::MessagePayload<
		AccountIdOf<ThisChain<B>>,
		SignerOf<BridgedChain<B>>,
		SignatureOf<BridgedChain<B>>,
		BridgedChainOpaqueCall,
	>;

	/// Messages delivery proof from bridged chain:
	///
	/// - hash of finalized header;
	/// - storage proof of inbound lane state;
	/// - lane id.
	#[derive(Clone, Decode, Encode, Eq, PartialEq, RuntimeDebug)]
	pub struct FromBridgedChainMessagesDeliveryProof<BridgedHeaderHash> {
		/// Hash of the bridge header the proof is for.
		pub bridged_header_hash: BridgedHeaderHash,
		/// Storage trie proof generated for [`Self::bridged_header_hash`].
		pub storage_proof: RawStorageProof,
		/// Lane id of which messages were delivered and the proof is for.
		pub lane: LaneId,
	}

	impl<BridgedHeaderHash> Size for FromBridgedChainMessagesDeliveryProof<BridgedHeaderHash> {
		fn size_hint(&self) -> u32 {
			u32::try_from(
				self.storage_proof
					.iter()
					.fold(0usize, |sum, node| sum.saturating_add(node.len())),
			)
			.unwrap_or(u32::MAX)
		}
	}

	/// 'Parsed' message delivery proof - inbound lane id and its state.
	pub type ParsedMessagesDeliveryProofFromBridgedChain<B> = (LaneId, InboundLaneData<AccountIdOf<ThisChain<B>>>);

	/// Message verifier that is doing all basic checks.
	///
	/// This verifier assumes following:
	///
	/// - all message lanes are equivalent, so all checks are the same;
	/// - messages are being dispatched using `pallet-bridge-dispatch` pallet on the target chain.
	///
	/// Following checks are made:
	///
	/// - message is rejected if its lane is currently blocked;
	/// - message is rejected if there are too many pending (undelivered) messages at the outbound lane;
	/// - check that the sender has rights to dispatch the call on target chain using provided dispatch origin;
	/// - check that the sender has paid enough funds for both message delivery and dispatch.
	#[derive(RuntimeDebug)]
	pub struct FromThisChainMessageVerifier<B>(PhantomData<B>);

	pub(crate) const OUTBOUND_LANE_DISABLED: &str = "The outbound message lane is disabled.";
	pub(crate) const TOO_MANY_PENDING_MESSAGES: &str = "Too many pending messages at the lane.";
	pub(crate) const BAD_ORIGIN: &str = "Unable to match the source origin to expected target origin.";
	pub(crate) const TOO_LOW_FEE: &str = "Provided fee is below minimal threshold required by the lane.";

	impl<B> LaneMessageVerifier<AccountIdOf<ThisChain<B>>, FromThisChainMessagePayload<B>, BalanceOf<ThisChain<B>>>
		for FromThisChainMessageVerifier<B>
	where
		B: MessageBridge,
		AccountIdOf<ThisChain<B>>: PartialEq + Clone,
	{
		type Error = &'static str;

		fn verify_message(
			submitter: &Sender<AccountIdOf<ThisChain<B>>>,
			delivery_and_dispatch_fee: &BalanceOf<ThisChain<B>>,
			lane: &LaneId,
			lane_outbound_data: &OutboundLaneData,
			payload: &FromThisChainMessagePayload<B>,
		) -> Result<(), Self::Error> {
			// reject message if lane is blocked
			if !ThisChain::<B>::is_outbound_lane_enabled(lane) {
				return Err(OUTBOUND_LANE_DISABLED);
			}

			// reject message if there are too many pending messages at this lane
			let max_pending_messages = ThisChain::<B>::maximal_pending_messages_at_outbound_lane();
			let pending_messages = lane_outbound_data
				.latest_generated_nonce
				.saturating_sub(lane_outbound_data.latest_received_nonce);
			if pending_messages > max_pending_messages {
				return Err(TOO_MANY_PENDING_MESSAGES);
			}

			// Do the dispatch-specific check. We assume that the target chain uses
			// `Dispatch`, so we verify the message accordingly.
			pallet_bridge_dispatch::verify_message_origin(submitter, payload).map_err(|_| BAD_ORIGIN)?;

			let minimal_fee_in_this_tokens =
				estimate_message_dispatch_and_delivery_fee::<B>(payload, B::RELAYER_FEE_PERCENT)?;

			// compare with actual fee paid
			if *delivery_and_dispatch_fee < minimal_fee_in_this_tokens {
				return Err(TOO_LOW_FEE);
			}

			Ok(())
		}
	}

	/// Return maximal message size of This -> Bridged chain message.
	pub fn maximal_message_size<B: MessageBridge>() -> u32 {
		super::target::maximal_incoming_message_size(BridgedChain::<B>::maximal_extrinsic_size())
	}

	/// Do basic Bridged-chain specific verification of This -> Bridged chain message.
	///
	/// Ok result from this function means that the delivery transaction with this message
	/// may be 'mined' by the target chain. But the lane may have its own checks (e.g. fee
	/// check) that would reject message (see `FromThisChainMessageVerifier`).
	pub fn verify_chain_message<B: MessageBridge>(
		payload: &FromThisChainMessagePayload<B>,
	) -> Result<(), &'static str> {
		let weight_limits = BridgedChain::<B>::message_weight_limits(&payload.call);
		if !weight_limits.contains(&payload.weight.into()) {
			return Err("Incorrect message weight declared");
		}

		// The maximal size of extrinsic at Substrate-based chain depends on the
		// `frame_system::Config::MaximumBlockLength` and `frame_system::Config::AvailableBlockRatio`
		// constants. This check is here to be sure that the lane won't stuck because message is too
		// large to fit into delivery transaction.
		//
		// **IMPORTANT NOTE**: the delivery transaction contains storage proof of the message, not
		// the message itself. The proof is always larger than the message. But unless chain state
		// is enormously large, it should be several dozens/hundreds of bytes. The delivery
		// transaction also contains signatures and signed extensions. Because of this, we reserve
		// 1/3 of the the maximal extrinsic weight for this data.
		if payload.call.len() > maximal_message_size::<B>() as usize {
			return Err("The message is too large to be sent over the lane");
		}

		Ok(())
	}

	/// Estimate delivery and dispatch fee that must be paid for delivering a message to the Bridged chain.
	///
	/// The fee is paid in This chain Balance, but we use Bridged chain balance to avoid additional conversions.
	/// Returns `None` if overflow has happened.
	pub fn estimate_message_dispatch_and_delivery_fee<B: MessageBridge>(
		payload: &FromThisChainMessagePayload<B>,
		relayer_fee_percent: u32,
	) -> Result<BalanceOf<ThisChain<B>>, &'static str> {
		// the fee (in Bridged tokens) of all transactions that are made on the Bridged chain
		//
		// if we're going to pay dispatch fee at the target chain, then we don't include weight
		// of the message dispatch in the delivery transaction cost
		let pay_dispatch_fee_at_target_chain = payload.dispatch_fee_payment == DispatchFeePayment::AtTargetChain;
		let delivery_transaction = BridgedChain::<B>::estimate_delivery_transaction(
			&payload.encode(),
			pay_dispatch_fee_at_target_chain,
			if pay_dispatch_fee_at_target_chain {
				0.into()
			} else {
				payload.weight.into()
			},
		);
		let delivery_transaction_fee = BridgedChain::<B>::transaction_payment(delivery_transaction);

		// the fee (in This tokens) of all transactions that are made on This chain
		let confirmation_transaction = ThisChain::<B>::estimate_delivery_confirmation_transaction();
		let confirmation_transaction_fee = ThisChain::<B>::transaction_payment(confirmation_transaction);

		// minimal fee (in This tokens) is a sum of all required fees
		let minimal_fee =
			B::bridged_balance_to_this_balance(delivery_transaction_fee).checked_add(&confirmation_transaction_fee);

		// before returning, add extra fee that is paid to the relayer (relayer interest)
		minimal_fee
			.and_then(|fee|
			// having message with fee that is near the `Balance::MAX_VALUE` of the chain is
			// unlikely and should be treated as an error
			// => let's do multiplication first
			fee
				.checked_mul(&relayer_fee_percent.into())
				.and_then(|interest| interest.checked_div(&100u32.into()))
				.and_then(|interest| fee.checked_add(&interest)))
			.ok_or("Overflow when computing minimal required message delivery and dispatch fee")
	}

	/// Verify proof of This -> Bridged chain messages delivery.
	pub fn verify_messages_delivery_proof<B: MessageBridge, ThisRuntime, GrandpaInstance: 'static>(
		proof: FromBridgedChainMessagesDeliveryProof<HashOf<BridgedChain<B>>>,
	) -> Result<ParsedMessagesDeliveryProofFromBridgedChain<B>, &'static str>
	where
		ThisRuntime: pallet_bridge_grandpa::Config<GrandpaInstance>,
		HashOf<BridgedChain<B>>:
			Into<bp_runtime::HashOf<<ThisRuntime as pallet_bridge_grandpa::Config<GrandpaInstance>>::BridgedChain>>,
	{
		let FromBridgedChainMessagesDeliveryProof {
			bridged_header_hash,
			storage_proof,
			lane,
		} = proof;
		pallet_bridge_grandpa::Pallet::<ThisRuntime, GrandpaInstance>::parse_finalized_storage_proof(
			bridged_header_hash.into(),
			StorageProof::new(storage_proof),
			|storage| {
				// Messages delivery proof is just proof of single storage key read => any error
				// is fatal.
				let storage_inbound_lane_data_key =
					pallet_bridge_messages::storage_keys::inbound_lane_data_key(B::BRIDGED_MESSAGES_PALLET_NAME, &lane);
				let raw_inbound_lane_data = storage
					.read_value(storage_inbound_lane_data_key.0.as_ref())
					.map_err(|_| "Failed to read inbound lane state from storage proof")?
					.ok_or("Inbound lane state is missing from the messages proof")?;
				let inbound_lane_data = InboundLaneData::decode(&mut &raw_inbound_lane_data[..])
					.map_err(|_| "Failed to decode inbound lane state from the proof")?;

				Ok((lane, inbound_lane_data))
			},
		)
		.map_err(<&'static str>::from)?
	}
}

/// Sub-module that is declaring types required for processing Bridged -> This chain messages.
pub mod target {
	use super::*;

	/// Call origin for Bridged -> This chain messages.
	pub type FromBridgedChainMessageCallOrigin<B> = bp_message_dispatch::CallOrigin<
		AccountIdOf<BridgedChain<B>>,
		SignerOf<ThisChain<B>>,
		SignatureOf<ThisChain<B>>,
	>;

	/// Decoded Bridged -> This message payload.
	pub type FromBridgedChainMessagePayload<B> = bp_message_dispatch::MessagePayload<
		AccountIdOf<BridgedChain<B>>,
		SignerOf<ThisChain<B>>,
		SignatureOf<ThisChain<B>>,
		FromBridgedChainEncodedMessageCall<CallOf<ThisChain<B>>>,
	>;

	/// Messages proof from bridged chain:
	///
	/// - hash of finalized header;
	/// - storage proof of messages and (optionally) outbound lane state;
	/// - lane id;
	/// - nonces (inclusive range) of messages which are included in this proof.
	#[derive(Clone, Decode, Encode, Eq, PartialEq, RuntimeDebug)]
	pub struct FromBridgedChainMessagesProof<BridgedHeaderHash> {
		/// Hash of the finalized bridged header the proof is for.
		pub bridged_header_hash: BridgedHeaderHash,
		/// A storage trie proof of messages being delivered.
		pub storage_proof: RawStorageProof,
		pub lane: LaneId,
		/// Nonce of the first message being delivered.
		pub nonces_start: MessageNonce,
		/// Nonce of the last message being delivered.
		pub nonces_end: MessageNonce,
	}

	impl<BridgedHeaderHash> Size for FromBridgedChainMessagesProof<BridgedHeaderHash> {
		fn size_hint(&self) -> u32 {
			u32::try_from(
				self.storage_proof
					.iter()
					.fold(0usize, |sum, node| sum.saturating_add(node.len())),
			)
			.unwrap_or(u32::MAX)
		}
	}

	/// Encoded Call of This chain as it is transferred over bridge.
	///
	/// Our Call is opaque (`Vec<u8>`) for Bridged chain. So it is encoded, prefixed with
	/// vector length. Custom decode implementation here is exactly to deal with this.
	#[derive(Decode, Encode, RuntimeDebug, PartialEq)]
	pub struct FromBridgedChainEncodedMessageCall<DecodedCall> {
		encoded_call: Vec<u8>,
		_marker: PhantomData<DecodedCall>,
	}

	impl<DecodedCall> FromBridgedChainEncodedMessageCall<DecodedCall> {
		/// Create encoded call.
		pub fn new(encoded_call: Vec<u8>) -> Self {
			FromBridgedChainEncodedMessageCall {
				encoded_call,
				_marker: PhantomData::default(),
			}
		}
	}

	impl<DecodedCall: Decode> From<FromBridgedChainEncodedMessageCall<DecodedCall>> for Result<DecodedCall, ()> {
		fn from(encoded_call: FromBridgedChainEncodedMessageCall<DecodedCall>) -> Self {
			DecodedCall::decode(&mut &encoded_call.encoded_call[..]).map_err(drop)
		}
	}

	/// Dispatching Bridged -> This chain messages.
	#[derive(RuntimeDebug, Clone, Copy)]
	pub struct FromBridgedChainMessageDispatch<B, ThisRuntime, ThisCurrency, ThisDispatchInstance> {
		_marker: PhantomData<(B, ThisRuntime, ThisCurrency, ThisDispatchInstance)>,
	}

	impl<B: MessageBridge, ThisRuntime, ThisCurrency, ThisDispatchInstance>
		MessageDispatch<AccountIdOf<ThisChain<B>>, BalanceOf<BridgedChain<B>>>
		for FromBridgedChainMessageDispatch<B, ThisRuntime, ThisCurrency, ThisDispatchInstance>
	where
		BalanceOf<ThisChain<B>>: Saturating + FixedPointOperand,
		ThisDispatchInstance: 'static,
		ThisRuntime: pallet_bridge_dispatch::Config<ThisDispatchInstance, MessageId = (LaneId, MessageNonce)>
			+ pallet_transaction_payment::Config,
		<ThisRuntime as pallet_transaction_payment::Config>::OnChargeTransaction:
			pallet_transaction_payment::OnChargeTransaction<ThisRuntime, Balance = BalanceOf<ThisChain<B>>>,
		ThisCurrency: Currency<AccountIdOf<ThisChain<B>>, Balance = BalanceOf<ThisChain<B>>>,
		pallet_bridge_dispatch::Pallet<ThisRuntime, ThisDispatchInstance>: bp_message_dispatch::MessageDispatch<
			AccountIdOf<ThisChain<B>>,
			(LaneId, MessageNonce),
			Message = FromBridgedChainMessagePayload<B>,
		>,
	{
		type DispatchPayload = FromBridgedChainMessagePayload<B>;

		fn dispatch_weight(
			message: &DispatchMessage<Self::DispatchPayload, BalanceOf<BridgedChain<B>>>,
		) -> frame_support::weights::Weight {
			message.data.payload.as_ref().map(|payload| payload.weight).unwrap_or(0)
		}

		fn dispatch(
			relayer_account: &AccountIdOf<ThisChain<B>>,
			message: DispatchMessage<Self::DispatchPayload, BalanceOf<BridgedChain<B>>>,
		) -> MessageDispatchResult {
			let message_id = (message.key.lane_id, message.key.nonce);
			pallet_bridge_dispatch::Pallet::<ThisRuntime, ThisDispatchInstance>::dispatch(
				B::BRIDGED_CHAIN_ID,
				B::THIS_CHAIN_ID,
				message_id,
				message.data.payload.map_err(drop),
				|dispatch_origin, dispatch_weight| {
					let unadjusted_weight_fee = ThisRuntime::WeightToFee::calc(&dispatch_weight);
					let fee_multiplier = pallet_transaction_payment::Pallet::<ThisRuntime>::next_fee_multiplier();
					let adjusted_weight_fee = fee_multiplier.saturating_mul_int(unadjusted_weight_fee);
					if !adjusted_weight_fee.is_zero() {
						ThisCurrency::transfer(
							dispatch_origin,
							relayer_account,
							adjusted_weight_fee,
							ExistenceRequirement::AllowDeath,
						)
						.map_err(drop)
					} else {
						Ok(())
					}
				},
			)
		}
	}

	/// Return maximal dispatch weight of the message we're able to receive.
	pub fn maximal_incoming_message_dispatch_weight(maximal_extrinsic_weight: Weight) -> Weight {
		maximal_extrinsic_weight / 2
	}

	/// Return maximal message size given maximal extrinsic size.
	pub fn maximal_incoming_message_size(maximal_extrinsic_size: u32) -> u32 {
		maximal_extrinsic_size / 3 * 2
	}

	/// Verify proof of Bridged -> This chain messages.
	///
	/// The `messages_count` argument verification (sane limits) is supposed to be made
	/// outside of this function. This function only verifies that the proof declares exactly
	/// `messages_count` messages.
	pub fn verify_messages_proof<B: MessageBridge, ThisRuntime, GrandpaInstance: 'static>(
		proof: FromBridgedChainMessagesProof<HashOf<BridgedChain<B>>>,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<BalanceOf<BridgedChain<B>>>>, &'static str>
	where
		ThisRuntime: pallet_bridge_grandpa::Config<GrandpaInstance>,
		HashOf<BridgedChain<B>>:
			Into<bp_runtime::HashOf<<ThisRuntime as pallet_bridge_grandpa::Config<GrandpaInstance>>::BridgedChain>>,
	{
		verify_messages_proof_with_parser::<B, _, _>(
			proof,
			messages_count,
			|bridged_header_hash, bridged_storage_proof| {
				pallet_bridge_grandpa::Pallet::<ThisRuntime, GrandpaInstance>::parse_finalized_storage_proof(
					bridged_header_hash.into(),
					StorageProof::new(bridged_storage_proof),
					|storage_adapter| storage_adapter,
				)
				.map(|storage| StorageProofCheckerAdapter::<_, B> {
					storage,
					_dummy: Default::default(),
				})
				.map_err(|err| MessageProofError::Custom(err.into()))
			},
		)
		.map_err(Into::into)
	}

	#[derive(Debug, PartialEq)]
	pub(crate) enum MessageProofError {
		Empty,
		MessagesCountMismatch,
		MissingRequiredMessage,
		FailedToDecodeMessage,
		FailedToDecodeOutboundLaneState,
		Custom(&'static str),
	}

	impl From<MessageProofError> for &'static str {
		fn from(err: MessageProofError) -> &'static str {
			match err {
				MessageProofError::Empty => "Messages proof is empty",
				MessageProofError::MessagesCountMismatch => "Declared messages count doesn't match actual value",
				MessageProofError::MissingRequiredMessage => "Message is missing from the proof",
				MessageProofError::FailedToDecodeMessage => "Failed to decode message from the proof",
				MessageProofError::FailedToDecodeOutboundLaneState => {
					"Failed to decode outbound lane data from the proof"
				}
				MessageProofError::Custom(err) => err,
			}
		}
	}

	pub(crate) trait MessageProofParser {
		fn read_raw_outbound_lane_data(&self, lane_id: &LaneId) -> Option<Vec<u8>>;
		fn read_raw_message(&self, message_key: &MessageKey) -> Option<Vec<u8>>;
	}

	struct StorageProofCheckerAdapter<H: Hasher, B> {
		storage: StorageProofChecker<H>,
		_dummy: sp_std::marker::PhantomData<B>,
	}

	impl<H, B> MessageProofParser for StorageProofCheckerAdapter<H, B>
	where
		H: Hasher,
		B: MessageBridge,
	{
		fn read_raw_outbound_lane_data(&self, lane_id: &LaneId) -> Option<Vec<u8>> {
			let storage_outbound_lane_data_key =
				pallet_bridge_messages::storage_keys::outbound_lane_data_key(B::BRIDGED_MESSAGES_PALLET_NAME, lane_id);
			self.storage
				.read_value(storage_outbound_lane_data_key.0.as_ref())
				.ok()?
		}

		fn read_raw_message(&self, message_key: &MessageKey) -> Option<Vec<u8>> {
			let storage_message_key = pallet_bridge_messages::storage_keys::message_key(
				B::BRIDGED_MESSAGES_PALLET_NAME,
				&message_key.lane_id,
				message_key.nonce,
			);
			self.storage.read_value(storage_message_key.0.as_ref()).ok()?
		}
	}

	/// Verify proof of Bridged -> This chain messages using given message proof parser.
	pub(crate) fn verify_messages_proof_with_parser<B: MessageBridge, BuildParser, Parser>(
		proof: FromBridgedChainMessagesProof<HashOf<BridgedChain<B>>>,
		messages_count: u32,
		build_parser: BuildParser,
	) -> Result<ProvedMessages<Message<BalanceOf<BridgedChain<B>>>>, MessageProofError>
	where
		BuildParser: FnOnce(HashOf<BridgedChain<B>>, RawStorageProof) -> Result<Parser, MessageProofError>,
		Parser: MessageProofParser,
	{
		let FromBridgedChainMessagesProof {
			bridged_header_hash,
			storage_proof,
			lane,
			nonces_start,
			nonces_end,
		} = proof;

		// receiving proofs where end < begin is ok (if proof includes outbound lane state)
		let messages_in_the_proof = if let Some(nonces_difference) = nonces_end.checked_sub(nonces_start) {
			// let's check that the user (relayer) has passed correct `messages_count`
			// (this bounds maximal capacity of messages vec below)
			let messages_in_the_proof = nonces_difference.saturating_add(1);
			if messages_in_the_proof != MessageNonce::from(messages_count) {
				return Err(MessageProofError::MessagesCountMismatch);
			}

			messages_in_the_proof
		} else {
			0
		};

		let parser = build_parser(bridged_header_hash, storage_proof)?;

		// Read messages first. All messages that are claimed to be in the proof must
		// be in the proof. So any error in `read_value`, or even missing value is fatal.
		//
		// Mind that we allow proofs with no messages if outbound lane state is proved.
		let mut messages = Vec::with_capacity(messages_in_the_proof as _);
		for nonce in nonces_start..=nonces_end {
			let message_key = MessageKey { lane_id: lane, nonce };
			let raw_message_data = parser
				.read_raw_message(&message_key)
				.ok_or(MessageProofError::MissingRequiredMessage)?;
			let message_data = MessageData::<BalanceOf<BridgedChain<B>>>::decode(&mut &raw_message_data[..])
				.map_err(|_| MessageProofError::FailedToDecodeMessage)?;
			messages.push(Message {
				key: message_key,
				data: message_data,
			});
		}

		// Now let's check if proof contains outbound lane state proof. It is optional, so we
		// simply ignore `read_value` errors and missing value.
		let mut proved_lane_messages = ProvedLaneMessages {
			lane_state: None,
			messages,
		};
		let raw_outbound_lane_data = parser.read_raw_outbound_lane_data(&lane);
		if let Some(raw_outbound_lane_data) = raw_outbound_lane_data {
			proved_lane_messages.lane_state = Some(
				OutboundLaneData::decode(&mut &raw_outbound_lane_data[..])
					.map_err(|_| MessageProofError::FailedToDecodeOutboundLaneState)?,
			);
		}

		// Now we may actually check if the proof is empty or not.
		if proved_lane_messages.lane_state.is_none() && proved_lane_messages.messages.is_empty() {
			return Err(MessageProofError::Empty);
		}

		// We only support single lane messages in this schema
		let mut proved_messages = ProvedMessages::new();
		proved_messages.insert(lane, proved_lane_messages);

		Ok(proved_messages)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use codec::{Decode, Encode};
	use frame_support::weights::Weight;
	use std::ops::RangeInclusive;

	const DELIVERY_TRANSACTION_WEIGHT: Weight = 100;
	const DELIVERY_CONFIRMATION_TRANSACTION_WEIGHT: Weight = 100;
	const THIS_CHAIN_WEIGHT_TO_BALANCE_RATE: Weight = 2;
	const BRIDGED_CHAIN_WEIGHT_TO_BALANCE_RATE: Weight = 4;
	const BRIDGED_CHAIN_TO_THIS_CHAIN_BALANCE_RATE: u32 = 6;
	const BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT: Weight = 2048;
	const BRIDGED_CHAIN_MAX_EXTRINSIC_SIZE: u32 = 1024;

	/// Bridge that is deployed on ThisChain and allows sending/receiving messages to/from BridgedChain;
	#[derive(Debug, PartialEq, Eq)]
	struct OnThisChainBridge;

	impl MessageBridge for OnThisChainBridge {
		const RELAYER_FEE_PERCENT: u32 = 10;
		const THIS_CHAIN_ID: ChainId = *b"this";
		const BRIDGED_CHAIN_ID: ChainId = *b"brdg";
		const BRIDGED_MESSAGES_PALLET_NAME: &'static str = "";

		type ThisChain = ThisChain;
		type BridgedChain = BridgedChain;

		fn bridged_balance_to_this_balance(bridged_balance: BridgedChainBalance) -> ThisChainBalance {
			ThisChainBalance(bridged_balance.0 * BRIDGED_CHAIN_TO_THIS_CHAIN_BALANCE_RATE as u32)
		}
	}

	/// Bridge that is deployed on BridgedChain and allows sending/receiving messages to/from ThisChain;
	#[derive(Debug, PartialEq, Eq)]
	struct OnBridgedChainBridge;

	impl MessageBridge for OnBridgedChainBridge {
		const RELAYER_FEE_PERCENT: u32 = 20;
		const THIS_CHAIN_ID: ChainId = *b"brdg";
		const BRIDGED_CHAIN_ID: ChainId = *b"this";
		const BRIDGED_MESSAGES_PALLET_NAME: &'static str = "";

		type ThisChain = BridgedChain;
		type BridgedChain = ThisChain;

		fn bridged_balance_to_this_balance(_this_balance: ThisChainBalance) -> BridgedChainBalance {
			unreachable!()
		}
	}

	#[derive(Debug, PartialEq, Decode, Encode, Clone)]
	struct ThisChainAccountId(u32);
	#[derive(Debug, PartialEq, Decode, Encode)]
	struct ThisChainSigner(u32);
	#[derive(Debug, PartialEq, Decode, Encode)]
	struct ThisChainSignature(u32);
	#[derive(Debug, PartialEq, Decode, Encode)]
	enum ThisChainCall {
		#[codec(index = 42)]
		Transfer,
		#[codec(index = 84)]
		Mint,
	}

	#[derive(Debug, PartialEq, Decode, Encode)]
	struct BridgedChainAccountId(u32);
	#[derive(Debug, PartialEq, Decode, Encode)]
	struct BridgedChainSigner(u32);
	#[derive(Debug, PartialEq, Decode, Encode)]
	struct BridgedChainSignature(u32);
	#[derive(Debug, PartialEq, Decode, Encode)]
	enum BridgedChainCall {}

	macro_rules! impl_wrapped_balance {
		($name:ident) => {
			#[derive(Debug, PartialEq, Decode, Encode, Clone, Copy)]
			struct $name(u32);

			impl From<u32> for $name {
				fn from(balance: u32) -> Self {
					Self(balance)
				}
			}

			impl sp_std::ops::Add for $name {
				type Output = $name;

				fn add(self, other: Self) -> Self {
					Self(self.0 + other.0)
				}
			}

			impl sp_std::ops::Div for $name {
				type Output = $name;

				fn div(self, other: Self) -> Self {
					Self(self.0 / other.0)
				}
			}

			impl sp_std::ops::Mul for $name {
				type Output = $name;

				fn mul(self, other: Self) -> Self {
					Self(self.0 * other.0)
				}
			}

			impl sp_std::cmp::PartialOrd for $name {
				fn partial_cmp(&self, other: &Self) -> Option<sp_std::cmp::Ordering> {
					self.0.partial_cmp(&other.0)
				}
			}

			impl CheckedAdd for $name {
				fn checked_add(&self, other: &Self) -> Option<Self> {
					self.0.checked_add(other.0).map(Self)
				}
			}

			impl CheckedDiv for $name {
				fn checked_div(&self, other: &Self) -> Option<Self> {
					self.0.checked_div(other.0).map(Self)
				}
			}

			impl CheckedMul for $name {
				fn checked_mul(&self, other: &Self) -> Option<Self> {
					self.0.checked_mul(other.0).map(Self)
				}
			}
		};
	}

	impl_wrapped_balance!(ThisChainBalance);
	impl_wrapped_balance!(BridgedChainBalance);

	struct ThisChain;

	impl ChainWithMessages for ThisChain {
		type Hash = ();
		type AccountId = ThisChainAccountId;
		type Signer = ThisChainSigner;
		type Signature = ThisChainSignature;
		type Weight = frame_support::weights::Weight;
		type Balance = ThisChainBalance;
	}

	impl ThisChainWithMessages for ThisChain {
		type Call = ThisChainCall;

		fn is_outbound_lane_enabled(lane: &LaneId) -> bool {
			lane == TEST_LANE_ID
		}

		fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
			MAXIMAL_PENDING_MESSAGES_AT_TEST_LANE
		}

		fn estimate_delivery_confirmation_transaction() -> MessageTransaction<WeightOf<Self>> {
			MessageTransaction {
				dispatch_weight: DELIVERY_CONFIRMATION_TRANSACTION_WEIGHT,
				size: 0,
			}
		}

		fn transaction_payment(transaction: MessageTransaction<WeightOf<Self>>) -> BalanceOf<Self> {
			ThisChainBalance(transaction.dispatch_weight as u32 * THIS_CHAIN_WEIGHT_TO_BALANCE_RATE as u32)
		}
	}

	impl BridgedChainWithMessages for ThisChain {
		fn maximal_extrinsic_size() -> u32 {
			unreachable!()
		}

		fn message_weight_limits(_message_payload: &[u8]) -> RangeInclusive<Self::Weight> {
			unreachable!()
		}

		fn estimate_delivery_transaction(
			_message_payload: &[u8],
			_include_pay_dispatch_fee_cost: bool,
			_message_dispatch_weight: WeightOf<Self>,
		) -> MessageTransaction<WeightOf<Self>> {
			unreachable!()
		}

		fn transaction_payment(_transaction: MessageTransaction<WeightOf<Self>>) -> BalanceOf<Self> {
			unreachable!()
		}
	}

	struct BridgedChain;

	impl ChainWithMessages for BridgedChain {
		type Hash = ();
		type AccountId = BridgedChainAccountId;
		type Signer = BridgedChainSigner;
		type Signature = BridgedChainSignature;
		type Weight = frame_support::weights::Weight;
		type Balance = BridgedChainBalance;
	}

	impl ThisChainWithMessages for BridgedChain {
		type Call = BridgedChainCall;

		fn is_outbound_lane_enabled(_lane: &LaneId) -> bool {
			unreachable!()
		}

		fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
			unreachable!()
		}

		fn estimate_delivery_confirmation_transaction() -> MessageTransaction<WeightOf<Self>> {
			unreachable!()
		}

		fn transaction_payment(_transaction: MessageTransaction<WeightOf<Self>>) -> BalanceOf<Self> {
			unreachable!()
		}
	}

	impl BridgedChainWithMessages for BridgedChain {
		fn maximal_extrinsic_size() -> u32 {
			BRIDGED_CHAIN_MAX_EXTRINSIC_SIZE
		}

		fn message_weight_limits(message_payload: &[u8]) -> RangeInclusive<Self::Weight> {
			let begin = std::cmp::min(BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT, message_payload.len() as Weight);
			begin..=BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT
		}

		fn estimate_delivery_transaction(
			_message_payload: &[u8],
			_include_pay_dispatch_fee_cost: bool,
			message_dispatch_weight: WeightOf<Self>,
		) -> MessageTransaction<WeightOf<Self>> {
			MessageTransaction {
				dispatch_weight: DELIVERY_TRANSACTION_WEIGHT + message_dispatch_weight,
				size: 0,
			}
		}

		fn transaction_payment(transaction: MessageTransaction<WeightOf<Self>>) -> BalanceOf<Self> {
			BridgedChainBalance(transaction.dispatch_weight as u32 * BRIDGED_CHAIN_WEIGHT_TO_BALANCE_RATE as u32)
		}
	}

	fn test_lane_outbound_data() -> OutboundLaneData {
		OutboundLaneData::default()
	}

	#[test]
	fn message_from_bridged_chain_is_decoded() {
		// the message is encoded on the bridged chain
		let message_on_bridged_chain = source::FromThisChainMessagePayload::<OnBridgedChainBridge> {
			spec_version: 1,
			weight: 100,
			origin: bp_message_dispatch::CallOrigin::SourceRoot,
			dispatch_fee_payment: DispatchFeePayment::AtTargetChain,
			call: ThisChainCall::Transfer.encode(),
		}
		.encode();

		// and sent to this chain where it is decoded
		let message_on_this_chain =
			target::FromBridgedChainMessagePayload::<OnThisChainBridge>::decode(&mut &message_on_bridged_chain[..])
				.unwrap();
		assert_eq!(
			message_on_this_chain,
			target::FromBridgedChainMessagePayload::<OnThisChainBridge> {
				spec_version: 1,
				weight: 100,
				origin: bp_message_dispatch::CallOrigin::SourceRoot,
				dispatch_fee_payment: DispatchFeePayment::AtTargetChain,
				call: target::FromBridgedChainEncodedMessageCall::<ThisChainCall>::new(
					ThisChainCall::Transfer.encode(),
				),
			}
		);
		assert_eq!(Ok(ThisChainCall::Transfer), message_on_this_chain.call.into());
	}

	const TEST_LANE_ID: &LaneId = b"test";
	const MAXIMAL_PENDING_MESSAGES_AT_TEST_LANE: MessageNonce = 32;

	fn regular_outbound_message_payload() -> source::FromThisChainMessagePayload<OnThisChainBridge> {
		source::FromThisChainMessagePayload::<OnThisChainBridge> {
			spec_version: 1,
			weight: 100,
			origin: bp_message_dispatch::CallOrigin::SourceRoot,
			dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
			call: vec![42],
		}
	}

	#[test]
	fn message_fee_is_checked_by_verifier() {
		const EXPECTED_MINIMAL_FEE: u32 = 5500;

		// payload of the This -> Bridged chain message
		let payload = regular_outbound_message_payload();

		// let's check if estimation matching hardcoded value
		assert_eq!(
			source::estimate_message_dispatch_and_delivery_fee::<OnThisChainBridge>(
				&payload,
				OnThisChainBridge::RELAYER_FEE_PERCENT,
			),
			Ok(ThisChainBalance(EXPECTED_MINIMAL_FEE)),
		);

		// let's check if estimation is less than hardcoded, if dispatch is paid at target chain
		let mut payload_with_pay_on_target = regular_outbound_message_payload();
		payload_with_pay_on_target.dispatch_fee_payment = DispatchFeePayment::AtTargetChain;
		let fee_at_source = source::estimate_message_dispatch_and_delivery_fee::<OnThisChainBridge>(
			&payload_with_pay_on_target,
			OnThisChainBridge::RELAYER_FEE_PERCENT,
		)
		.expect("estimate_message_dispatch_and_delivery_fee failed for pay-at-target-chain message");
		assert!(
			fee_at_source < EXPECTED_MINIMAL_FEE.into(),
			"Computed fee {:?} without prepaid dispatch must be less than the fee with prepaid dispatch {}",
			fee_at_source,
			EXPECTED_MINIMAL_FEE,
		);

		// and now check that the verifier checks the fee
		assert_eq!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::Root,
				&ThisChainBalance(1),
				TEST_LANE_ID,
				&test_lane_outbound_data(),
				&payload,
			),
			Err(source::TOO_LOW_FEE)
		);
		assert!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::Root,
				&ThisChainBalance(1_000_000),
				TEST_LANE_ID,
				&test_lane_outbound_data(),
				&payload,
			)
			.is_ok(),
		);
	}

	#[test]
	fn should_disallow_root_calls_from_regular_accounts() {
		// payload of the This -> Bridged chain message
		let payload = source::FromThisChainMessagePayload::<OnThisChainBridge> {
			spec_version: 1,
			weight: 100,
			origin: bp_message_dispatch::CallOrigin::SourceRoot,
			dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
			call: vec![42],
		};

		// and now check that the verifier checks the fee
		assert_eq!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::Signed(ThisChainAccountId(0)),
				&ThisChainBalance(1_000_000),
				TEST_LANE_ID,
				&test_lane_outbound_data(),
				&payload,
			),
			Err(source::BAD_ORIGIN)
		);
		assert_eq!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::None,
				&ThisChainBalance(1_000_000),
				TEST_LANE_ID,
				&test_lane_outbound_data(),
				&payload,
			),
			Err(source::BAD_ORIGIN)
		);
		assert!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::Root,
				&ThisChainBalance(1_000_000),
				TEST_LANE_ID,
				&test_lane_outbound_data(),
				&payload,
			)
			.is_ok(),
		);
	}

	#[test]
	fn should_verify_source_and_target_origin_matching() {
		// payload of the This -> Bridged chain message
		let payload = source::FromThisChainMessagePayload::<OnThisChainBridge> {
			spec_version: 1,
			weight: 100,
			origin: bp_message_dispatch::CallOrigin::SourceAccount(ThisChainAccountId(1)),
			dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
			call: vec![42],
		};

		// and now check that the verifier checks the fee
		assert_eq!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::Signed(ThisChainAccountId(0)),
				&ThisChainBalance(1_000_000),
				TEST_LANE_ID,
				&test_lane_outbound_data(),
				&payload,
			),
			Err(source::BAD_ORIGIN)
		);
		assert!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::Signed(ThisChainAccountId(1)),
				&ThisChainBalance(1_000_000),
				TEST_LANE_ID,
				&test_lane_outbound_data(),
				&payload,
			)
			.is_ok(),
		);
	}

	#[test]
	fn message_is_rejected_when_sent_using_disabled_lane() {
		assert_eq!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::Root,
				&ThisChainBalance(1_000_000),
				b"dsbl",
				&test_lane_outbound_data(),
				&regular_outbound_message_payload(),
			),
			Err(source::OUTBOUND_LANE_DISABLED)
		);
	}

	#[test]
	fn message_is_rejected_when_there_are_too_many_pending_messages_at_outbound_lane() {
		assert_eq!(
			source::FromThisChainMessageVerifier::<OnThisChainBridge>::verify_message(
				&Sender::Root,
				&ThisChainBalance(1_000_000),
				TEST_LANE_ID,
				&OutboundLaneData {
					latest_received_nonce: 100,
					latest_generated_nonce: 100 + MAXIMAL_PENDING_MESSAGES_AT_TEST_LANE + 1,
					..Default::default()
				},
				&regular_outbound_message_payload(),
			),
			Err(source::TOO_MANY_PENDING_MESSAGES)
		);
	}

	#[test]
	fn verify_chain_message_rejects_message_with_too_small_declared_weight() {
		assert!(
			source::verify_chain_message::<OnThisChainBridge>(&source::FromThisChainMessagePayload::<
				OnThisChainBridge,
			> {
				spec_version: 1,
				weight: 5,
				origin: bp_message_dispatch::CallOrigin::SourceRoot,
				dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
				call: vec![1, 2, 3, 4, 5, 6],
			},)
			.is_err()
		);
	}

	#[test]
	fn verify_chain_message_rejects_message_with_too_large_declared_weight() {
		assert!(
			source::verify_chain_message::<OnThisChainBridge>(&source::FromThisChainMessagePayload::<
				OnThisChainBridge,
			> {
				spec_version: 1,
				weight: BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT + 1,
				origin: bp_message_dispatch::CallOrigin::SourceRoot,
				dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
				call: vec![1, 2, 3, 4, 5, 6],
			},)
			.is_err()
		);
	}

	#[test]
	fn verify_chain_message_rejects_message_too_large_message() {
		assert!(
			source::verify_chain_message::<OnThisChainBridge>(&source::FromThisChainMessagePayload::<
				OnThisChainBridge,
			> {
				spec_version: 1,
				weight: BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT,
				origin: bp_message_dispatch::CallOrigin::SourceRoot,
				dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
				call: vec![0; source::maximal_message_size::<OnThisChainBridge>() as usize + 1],
			},)
			.is_err()
		);
	}

	#[test]
	fn verify_chain_message_accepts_maximal_message() {
		assert_eq!(
			source::verify_chain_message::<OnThisChainBridge>(&source::FromThisChainMessagePayload::<
				OnThisChainBridge,
			> {
				spec_version: 1,
				weight: BRIDGED_CHAIN_MAX_EXTRINSIC_WEIGHT,
				origin: bp_message_dispatch::CallOrigin::SourceRoot,
				dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
				call: vec![0; source::maximal_message_size::<OnThisChainBridge>() as _],
			},),
			Ok(()),
		);
	}

	#[derive(Debug)]
	struct TestMessageProofParser {
		failing: bool,
		messages: RangeInclusive<MessageNonce>,
		outbound_lane_data: Option<OutboundLaneData>,
	}

	impl target::MessageProofParser for TestMessageProofParser {
		fn read_raw_outbound_lane_data(&self, _lane_id: &LaneId) -> Option<Vec<u8>> {
			if self.failing {
				Some(vec![])
			} else {
				self.outbound_lane_data.clone().map(|data| data.encode())
			}
		}

		fn read_raw_message(&self, message_key: &MessageKey) -> Option<Vec<u8>> {
			if self.failing {
				Some(vec![])
			} else if self.messages.contains(&message_key.nonce) {
				Some(
					MessageData::<BridgedChainBalance> {
						payload: message_key.nonce.encode(),
						fee: BridgedChainBalance(0),
					}
					.encode(),
				)
			} else {
				None
			}
		}
	}

	#[allow(clippy::reversed_empty_ranges)]
	fn no_messages_range() -> RangeInclusive<MessageNonce> {
		1..=0
	}

	fn messages_proof(nonces_end: MessageNonce) -> target::FromBridgedChainMessagesProof<()> {
		target::FromBridgedChainMessagesProof {
			bridged_header_hash: (),
			storage_proof: vec![],
			lane: Default::default(),
			nonces_start: 1,
			nonces_end,
		}
	}

	#[test]
	fn messages_proof_is_rejected_if_declared_less_than_actual_number_of_messages() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, TestMessageProofParser>(
				messages_proof(10),
				5,
				|_, _| unreachable!(),
			),
			Err(target::MessageProofError::MessagesCountMismatch),
		);
	}

	#[test]
	fn messages_proof_is_rejected_if_declared_more_than_actual_number_of_messages() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, TestMessageProofParser>(
				messages_proof(10),
				15,
				|_, _| unreachable!(),
			),
			Err(target::MessageProofError::MessagesCountMismatch),
		);
	}

	#[test]
	fn message_proof_is_rejected_if_build_parser_fails() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, TestMessageProofParser>(
				messages_proof(10),
				10,
				|_, _| Err(target::MessageProofError::Custom("test")),
			),
			Err(target::MessageProofError::Custom("test")),
		);
	}

	#[test]
	fn message_proof_is_rejected_if_required_message_is_missing() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(messages_proof(10), 10, |_, _| Ok(
				TestMessageProofParser {
					failing: false,
					messages: 1..=5,
					outbound_lane_data: None,
				}
			),),
			Err(target::MessageProofError::MissingRequiredMessage),
		);
	}

	#[test]
	fn message_proof_is_rejected_if_message_decode_fails() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(messages_proof(10), 10, |_, _| Ok(
				TestMessageProofParser {
					failing: true,
					messages: 1..=10,
					outbound_lane_data: None,
				}
			),),
			Err(target::MessageProofError::FailedToDecodeMessage),
		);
	}

	#[test]
	fn message_proof_is_rejected_if_outbound_lane_state_decode_fails() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(messages_proof(0), 0, |_, _| Ok(
				TestMessageProofParser {
					failing: true,
					messages: no_messages_range(),
					outbound_lane_data: Some(OutboundLaneData {
						oldest_unpruned_nonce: 1,
						latest_received_nonce: 1,
						latest_generated_nonce: 1,
					}),
				}
			),),
			Err(target::MessageProofError::FailedToDecodeOutboundLaneState),
		);
	}

	#[test]
	fn message_proof_is_rejected_if_it_is_empty() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(messages_proof(0), 0, |_, _| Ok(
				TestMessageProofParser {
					failing: false,
					messages: no_messages_range(),
					outbound_lane_data: None,
				}
			),),
			Err(target::MessageProofError::Empty),
		);
	}

	#[test]
	fn non_empty_message_proof_without_messages_is_accepted() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(messages_proof(0), 0, |_, _| Ok(
				TestMessageProofParser {
					failing: false,
					messages: no_messages_range(),
					outbound_lane_data: Some(OutboundLaneData {
						oldest_unpruned_nonce: 1,
						latest_received_nonce: 1,
						latest_generated_nonce: 1,
					}),
				}
			),),
			Ok(vec![(
				Default::default(),
				ProvedLaneMessages {
					lane_state: Some(OutboundLaneData {
						oldest_unpruned_nonce: 1,
						latest_received_nonce: 1,
						latest_generated_nonce: 1,
					}),
					messages: Vec::new(),
				},
			)]
			.into_iter()
			.collect()),
		);
	}

	#[test]
	fn non_empty_message_proof_is_accepted() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(messages_proof(1), 1, |_, _| Ok(
				TestMessageProofParser {
					failing: false,
					messages: 1..=1,
					outbound_lane_data: Some(OutboundLaneData {
						oldest_unpruned_nonce: 1,
						latest_received_nonce: 1,
						latest_generated_nonce: 1,
					}),
				}
			),),
			Ok(vec![(
				Default::default(),
				ProvedLaneMessages {
					lane_state: Some(OutboundLaneData {
						oldest_unpruned_nonce: 1,
						latest_received_nonce: 1,
						latest_generated_nonce: 1,
					}),
					messages: vec![Message {
						key: MessageKey {
							lane_id: Default::default(),
							nonce: 1
						},
						data: MessageData {
							payload: 1u64.encode(),
							fee: BridgedChainBalance(0)
						},
					}],
				},
			)]
			.into_iter()
			.collect()),
		);
	}

	#[test]
	fn verify_messages_proof_with_parser_does_not_panic_if_messages_count_mismatches() {
		assert_eq!(
			target::verify_messages_proof_with_parser::<OnThisChainBridge, _, _>(
				messages_proof(u64::MAX),
				0,
				|_, _| Ok(TestMessageProofParser {
					failing: false,
					messages: 0..=u64::MAX,
					outbound_lane_data: Some(OutboundLaneData {
						oldest_unpruned_nonce: 1,
						latest_received_nonce: 1,
						latest_generated_nonce: 1,
					}),
				}),
			),
			Err(target::MessageProofError::MessagesCountMismatch),
		);
	}

	#[test]
	fn transaction_payment_works_with_zero_multiplier() {
		use sp_runtime::traits::Zero;

		assert_eq!(
			transaction_payment(
				100,
				10,
				FixedU128::zero(),
				|weight| weight,
				MessageTransaction {
					size: 50,
					dispatch_weight: 777
				},
			),
			100 + 50 * 10,
		);
	}

	#[test]
	fn transaction_payment_works_with_non_zero_multiplier() {
		use sp_runtime::traits::One;

		assert_eq!(
			transaction_payment(
				100,
				10,
				FixedU128::one(),
				|weight| weight,
				MessageTransaction {
					size: 50,
					dispatch_weight: 777
				},
			),
			100 + 50 * 10 + 777,
		);
	}
}
