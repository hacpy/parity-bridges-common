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

#![cfg_attr(not(feature = "std"), no_std)]
// RuntimeApi generated functions
#![allow(clippy::too_many_arguments)]
// Runtime-generated DecodeLimit::decode_all_with_depth_limit
#![allow(clippy::unnecessary_mut_passed)]

use bp_messages::{LaneId, MessageDetails, MessageNonce, UnrewardedRelayersState};
use sp_std::prelude::*;

pub use bp_polkadot_core::*;
// Rococo runtime = Wococo runtime
pub use bp_rococo::{WeightToFee, PAY_INBOUND_DISPATCH_FEE_WEIGHT, SESSION_LENGTH, VERSION};

/// Wococo Chain
pub type Wococo = PolkadotLike;

// We use this to get the account on Wococo (target) which is derived from Rococo's (source)
// account.
pub fn derive_account_from_rococo_id(id: bp_runtime::SourceAccount<AccountId>) -> AccountId {
	let encoded_id = bp_runtime::derive_account_id(bp_runtime::ROCOCO_CHAIN_ID, id);
	AccountIdConverter::convert(encoded_id)
}

/// Name of the With-Rococo messages pallet instance in the Wococo runtime.
pub const WITH_ROCOCO_MESSAGES_PALLET_NAME: &str = "BridgeRococoMessages";

/// Name of the `WococoFinalityApi::best_finalized` runtime method.
pub const BEST_FINALIZED_WOCOCO_HEADER_METHOD: &str = "WococoFinalityApi_best_finalized";
/// Name of the `WococoFinalityApi::is_known_header` runtime method.
pub const IS_KNOWN_WOCOCO_HEADER_METHOD: &str = "WococoFinalityApi_is_known_header";

/// Name of the `ToWococoOutboundLaneApi::estimate_message_delivery_and_dispatch_fee` runtime method.
pub const TO_WOCOCO_ESTIMATE_MESSAGE_FEE_METHOD: &str =
	"ToWococoOutboundLaneApi_estimate_message_delivery_and_dispatch_fee";
/// Name of the `ToWococoOutboundLaneApi::message_details` runtime method.
pub const TO_WOCOCO_MESSAGE_DETAILS_METHOD: &str = "ToWococoOutboundLaneApi_message_details";
/// Name of the `ToWococoOutboundLaneApi::latest_generated_nonce` runtime method.
pub const TO_WOCOCO_LATEST_GENERATED_NONCE_METHOD: &str = "ToWococoOutboundLaneApi_latest_generated_nonce";
/// Name of the `ToWococoOutboundLaneApi::latest_received_nonce` runtime method.
pub const TO_WOCOCO_LATEST_RECEIVED_NONCE_METHOD: &str = "ToWococoOutboundLaneApi_latest_received_nonce";

/// Name of the `FromWococoInboundLaneApi::latest_received_nonce` runtime method.
pub const FROM_WOCOCO_LATEST_RECEIVED_NONCE_METHOD: &str = "FromWococoInboundLaneApi_latest_received_nonce";
/// Name of the `FromWococoInboundLaneApi::latest_onfirmed_nonce` runtime method.
pub const FROM_WOCOCO_LATEST_CONFIRMED_NONCE_METHOD: &str = "FromWococoInboundLaneApi_latest_confirmed_nonce";
/// Name of the `FromWococoInboundLaneApi::unrewarded_relayers_state` runtime method.
pub const FROM_WOCOCO_UNREWARDED_RELAYERS_STATE: &str = "FromWococoInboundLaneApi_unrewarded_relayers_state";

sp_api::decl_runtime_apis! {
	/// API for querying information about the finalized Wococo headers.
	///
	/// This API is implemented by runtimes that are bridging with the Wococo chain, not the
	/// Wococo runtime itself.
	pub trait WococoFinalityApi {
		/// Returns number and hash of the best finalized header known to the bridge module.
		fn best_finalized() -> (BlockNumber, Hash);
		/// Returns true if the header is known to the runtime.
		fn is_known_header(hash: Hash) -> bool;
	}

	/// Outbound message lane API for messages that are sent to Wococo chain.
	///
	/// This API is implemented by runtimes that are sending messages to Wococo chain, not the
	/// Wococo runtime itself.
	pub trait ToWococoOutboundLaneApi<OutboundMessageFee: Parameter, OutboundPayload: Parameter> {
		/// Estimate message delivery and dispatch fee that needs to be paid by the sender on
		/// this chain.
		///
		/// Returns `None` if message is too expensive to be sent to Wococo from this chain.
		///
		/// Please keep in mind that this method returns the lowest message fee required for message
		/// to be accepted to the lane. It may be good idea to pay a bit over this price to account
		/// future exchange rate changes and guarantee that relayer would deliver your message
		/// to the target chain.
		fn estimate_message_delivery_and_dispatch_fee(
			lane_id: LaneId,
			payload: OutboundPayload,
		) -> Option<OutboundMessageFee>;
		/// Returns dispatch weight, encoded payload size and delivery+dispatch fee of all
		/// messages in given inclusive range.
		///
		/// If some (or all) messages are missing from the storage, they'll also will
		/// be missing from the resulting vector. The vector is ordered by the nonce.
		fn message_details(
			lane: LaneId,
			begin: MessageNonce,
			end: MessageNonce,
		) -> Vec<MessageDetails<OutboundMessageFee>>;
		/// Returns nonce of the latest message, received by bridged chain.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Returns nonce of the latest message, generated by given lane.
		fn latest_generated_nonce(lane: LaneId) -> MessageNonce;
	}

	/// Inbound message lane API for messages sent by Wococo chain.
	///
	/// This API is implemented by runtimes that are receiving messages from Wococo chain, not the
	/// Wococo runtime itself.
	pub trait FromWococoInboundLaneApi {
		/// Returns nonce of the latest message, received by given lane.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Nonce of the latest message that has been confirmed to the bridged chain.
		fn latest_confirmed_nonce(lane: LaneId) -> MessageNonce;
		/// State of the unrewarded relayers set at given lane.
		fn unrewarded_relayers_state(lane: LaneId) -> UnrewardedRelayersState;
	}
}
