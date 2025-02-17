use std::collections::HashMap;

use enumorph::Enumorph;
use frunk::{hlist_pat, HList};
use macros::model;
use num_bigint::BigUint;
use subset_of::SubsetOf;
use tracing::{debug, trace};
use unionlabs::{
    bounded::BoundedI64,
    cometbls::types::canonical_vote::CanonicalVote,
    ibc::{
        core::client::height::Height,
        lightclients::cometbls::{header::Header, light_header::LightHeader},
    },
    tendermint::{
        crypto::public_key::PublicKey,
        types::{
            canonical_block_header::CanonicalPartSetHeader, canonical_block_id::CanonicalBlockId,
            commit_sig::CommitSig, signed_header::SignedHeader, signed_msg_type::SignedMsgType,
            simple_validator::SimpleValidator,
        },
    },
    union::galois::{prove_request::ProveRequest, validator_set_commit::ValidatorSetCommit},
};
use voyager_message::{
    call::Call,
    callback::Callback,
    core::ChainId,
    data::{DecodedHeaderMeta, OrderedHeaders},
    PluginMessage, VoyagerMessage,
};
use voyager_vm::{aggregation::DoCallback, call, data, promise, Op};

use crate::{
    call::{FetchProveRequest, ModuleCall},
    data::{ModuleData, ProveResponse, TrustedValidators, UntrustedCommit, UntrustedValidators},
};

#[model]
#[derive(Enumorph, SubsetOf)]
#[allow(clippy::large_enum_variant)]
pub enum ModuleCallback {
    AggregateProveRequest(AggregateProveRequest),
    AggregateHeader(AggregateHeader),
}

#[model]
pub struct AggregateProveRequest {
    pub chain_id: ChainId<'static>,

    pub update_from: Height,
    pub update_to: Height,
}

#[model]
pub struct AggregateHeader {
    pub chain_id: ChainId<'static>,

    pub signed_header: SignedHeader,

    pub update_from: Height,
    pub update_to: Height,
}

impl DoCallback<VoyagerMessage<ModuleData, ModuleCall, ModuleCallback>> for AggregateProveRequest {
    type Params = HList![
        PluginMessage<UntrustedCommit>,
        PluginMessage<TrustedValidators>,
        PluginMessage<UntrustedValidators>
    ];

    fn call(
        AggregateProveRequest {
            chain_id,
            update_from,
            update_to,
        }: Self,
        hlist_pat![
            PluginMessage {
                plugin: plugin_name,
                message: UntrustedCommit {
                    height: _untrusted_commit_height,
                    signed_header,
                }
            },
            PluginMessage {
                plugin: _,
                message: TrustedValidators {
                    height: _trusted_validators_height,
                    validators: trusted_validators,
                }
            },
            PluginMessage {
                plugin: _,
                message: UntrustedValidators {
                    height: _untrusted_validators_height,
                    validators: untrusted_validators,
                }
            },
        ]: Self::Params,
    ) -> Op<VoyagerMessage<ModuleData, ModuleCall, ModuleCallback>> {
        let make_validators_commit =
            |mut validators: Vec<unionlabs::tendermint::types::validator::Validator>| {
                // Validators must be sorted to match the root, by token then address
                validators.sort_by(|a, b| {
                    // TODO: Double check how these comparisons are supposed to work
                    #[allow(clippy::collapsible_else_if)]
                    if a.voting_power == b.voting_power {
                        if a.address < b.address {
                            std::cmp::Ordering::Less
                        } else {
                            std::cmp::Ordering::Greater
                        }
                    } else {
                        if a.voting_power > b.voting_power {
                            std::cmp::Ordering::Less
                        } else {
                            std::cmp::Ordering::Greater
                        }
                    }
                });

                // The bitmap is a public input of the circuit, it must fit in Fr (scalar field) bn254
                let mut bitmap = BigUint::default();
                // REVIEW: This will over-allocate for the trusted validators; should be benchmarked
                let mut signatures = Vec::<Vec<u8>>::with_capacity(validators.len());

                let validators_map = validators
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (v.address, i))
                    .collect::<HashMap<_, _>>();

                // For each validator signature, we search for the actual validator
                // in the set and set it's signed bit to 1. We then push the
                // signature only if the validator signed. It's possible that we
                // don't find a validator for a given signature as the validator set
                // may have drifted (trusted validator set).
                for sig in signed_header.commit.signatures.iter() {
                    match sig {
                        CommitSig::Absent => {
                            debug!("validator did not sign");
                        }
                        CommitSig::Commit {
                            validator_address,
                            timestamp: _,
                            signature,
                        } => {
                            if let Some(validator_index) = validators_map.get(validator_address) {
                                bitmap.set_bit(*validator_index as u64, true);
                                signatures.push(signature.clone());
                                trace!(
                                    %validator_address,
                                    %validator_index,
                                    "validator signed"
                                );
                            } else {
                                trace!(
                                    %validator_address,
                                    "validator set drifted, could not find validator signature"
                                );
                            }
                        }
                        CommitSig::Nil {
                            validator_address, ..
                        } => {
                            trace!(
                                %validator_address,
                                "validator commit is nil"
                            );
                        }
                    }
                }

                let simple_validators = validators
                    .iter()
                    .map(|v| {
                        let PublicKey::Bn254(ref key) = v.pub_key else {
                            panic!("must be bn254")
                        };
                        SimpleValidator {
                            pub_key: PublicKey::Bn254(key.to_vec()),
                            voting_power: v.voting_power.into(),
                        }
                    })
                    .collect::<Vec<_>>();

                ValidatorSetCommit {
                    validators: simple_validators,
                    signatures,
                    bitmap: bitmap.to_bytes_be(),
                }
            };

        let trusted_validators_commit = make_validators_commit(trusted_validators);
        let untrusted_validators_commit = make_validators_commit(untrusted_validators);

        promise(
            [call(Call::plugin(
                &plugin_name,
                FetchProveRequest {
                    request: ProveRequest {
                        vote: CanonicalVote {
                            // REVIEW: Should this be hardcoded to precommit?
                            ty: SignedMsgType::Precommit,
                            height: signed_header.commit.height,
                            round: BoundedI64::new(signed_header.commit.round.inner().into())
                                .expect("0..=i32::MAX can be converted to 0..=i64::MAX safely"),
                            block_id: CanonicalBlockId {
                                hash: signed_header.commit.block_id.hash.unwrap_or_default(),
                                part_set_header: CanonicalPartSetHeader {
                                    total: signed_header.commit.block_id.part_set_header.total,
                                    hash: signed_header
                                        .commit
                                        .block_id
                                        .part_set_header
                                        .hash
                                        .unwrap_or_default(),
                                },
                            },
                            chain_id: signed_header.header.chain_id.clone(),
                        },
                        untrusted_header: signed_header.header.clone(),
                        trusted_commit: trusted_validators_commit,
                        untrusted_commit: untrusted_validators_commit,
                    },
                },
            ))],
            [],
            Callback::plugin(
                &plugin_name,
                AggregateHeader {
                    chain_id,
                    signed_header,
                    update_from,
                    update_to,
                },
            ),
        )
    }
}

impl DoCallback<VoyagerMessage<ModuleData, ModuleCall, ModuleCallback>> for AggregateHeader {
    type Params = HList![PluginMessage<ProveResponse>];

    fn call(
        AggregateHeader {
            mut signed_header,
            chain_id: _,
            update_from,
            update_to: _,
        }: Self,
        hlist_pat![PluginMessage {
            plugin: _,
            message: ProveResponse {
                prove_response: response,
            }
        }]: Self::Params,
    ) -> Op<VoyagerMessage<ModuleData, ModuleCall, ModuleCallback>> {
        // TODO: maybe introduce a new commit for union signed header as we don't need the signatures but the ZKP only
        // Keeping this signatures significantly increase the size of the structure and the associated gas cost in EVM (calldata).
        signed_header.commit.signatures.clear();

        data(OrderedHeaders {
            headers: vec![(
                DecodedHeaderMeta {
                    height: Height {
                        revision_number: update_from.revision_number,
                        revision_height: signed_header.header.height.inner().try_into().unwrap(),
                    },
                },
                serde_json::to_value(Header {
                    signed_header: LightHeader {
                        height: signed_header.header.height,
                        time: signed_header.header.time,
                        validators_hash: signed_header.header.validators_hash,
                        next_validators_hash: signed_header.header.next_validators_hash,
                        app_hash: signed_header.header.app_hash,
                    },
                    trusted_height: update_from,
                    zero_knowledge_proof: response.proof.evm_proof,
                })
                .unwrap(),
            )],
        })
    }
}
