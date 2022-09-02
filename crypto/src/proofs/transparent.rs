//! Transparent proofs for `MVP1` of the Penumbra system.

use anyhow::{anyhow, Error, Result};
use ark_ff::{PrimeField, Zero};
use std::convert::{TryFrom, TryInto};

use decaf377::FieldExt;
use decaf377_rdsa::{SpendAuth, VerificationKey};
use penumbra_proto::{transparent_proofs, Message, Protobuf};
use penumbra_tct as tct;

use crate::{
    asset,
    dex::{swap::SwapPlaintext, BatchSwapOutputData, TradingPair},
    fmd, ka, keys, note,
    transaction::Fee,
    value, Address, Fq, Fr, Nullifier, Value,
};

/// Transparent proof for spending existing notes.
///
/// This structure keeps track of the auxiliary (private) inputs.
#[derive(Clone, Debug)]
pub struct SpendProof {
    // Inclusion proof for the note commitment.
    pub note_commitment_proof: tct::Proof,
    // The diversified base for the address.
    pub g_d: decaf377::Element,
    // The transmission key for the address.
    pub pk_d: ka::Public,
    // The clue key for the address.
    pub ck_d: fmd::ClueKey,
    // The value of the note.
    pub value: Value,
    // The blinding factor used for generating the value commitment.
    pub v_blinding: Fr,
    // The blinding factor used for generating the note commitment.
    pub note_blinding: Fq,
    // The randomizer used for generating the randomized spend auth key.
    pub spend_auth_randomizer: Fr,
    // The spend authorization key.
    pub ak: VerificationKey<SpendAuth>,
    // The nullifier deriving key.
    pub nk: keys::NullifierKey,
}

impl SpendProof {
    /// Called to verify the proof using the provided public inputs.
    ///
    /// The public inputs are:
    /// * the merkle root of the note commitment tree,
    /// * value commitment of the note to be spent,
    /// * nullifier of the note to be spent,
    /// * the randomized verification spend key,
    pub fn verify(
        &self,
        anchor: tct::Root,
        value_commitment: value::Commitment,
        nullifier: Nullifier,
        rk: VerificationKey<SpendAuth>,
    ) -> anyhow::Result<()> {
        // Note commitment integrity.
        let s_component_transmission_key = Fq::from_bytes(self.pk_d.0);
        if let Ok(transmission_key_s) = s_component_transmission_key {
            let note_commitment_test = note::commitment(
                self.note_blinding,
                self.value,
                self.g_d,
                transmission_key_s,
                &self.ck_d,
            );

            if self.note_commitment_proof.commitment() != note_commitment_test {
                return Err(anyhow!("note commitment mismatch"));
            }
        } else {
            return Err(anyhow!("transmission key mismatch"));
        }

        // Merkle path integrity.
        self.note_commitment_proof
            .verify(anchor)
            .map_err(|_| anyhow!("merkle root mismatch"))?;

        // Value commitment integrity.
        if self.value.commit(self.v_blinding) != value_commitment {
            return Err(anyhow!("value commitment mismatch"));
        }

        // The use of decaf means that we do not need to check that the
        // diversified basepoint is of small order. However we instead
        // check it is not identity.
        if self.g_d.is_identity() || self.ak.is_identity() {
            return Err(anyhow!("unexpected identity"));
        }

        // Nullifier integrity.
        if nullifier
            != self.nk.derive_nullifier(
                self.note_commitment_proof.position(),
                &self.note_commitment_proof.commitment(),
            )
        {
            return Err(anyhow!("bad nullifier"));
        }

        // Spend authority.
        let rk_bytes: [u8; 32] = rk.into();
        let rk_test = self.ak.randomize(&self.spend_auth_randomizer);
        let rk_test_bytes: [u8; 32] = rk_test.into();
        if rk_bytes != rk_test_bytes {
            return Err(anyhow!("invalid spend auth randomizer"));
        }

        // Diversified address integrity.
        let fvk = keys::FullViewingKey::from_components(self.ak, self.nk);
        let ivk = fvk.incoming();
        if self.pk_d != ivk.diversified_public(&self.g_d) {
            return Err(anyhow!("invalid diversified address"));
        }

        Ok(())
    }
}

/// Transparent proof for new note creation.
///
/// This structure keeps track of the auxiliary (private) inputs.
#[derive(Clone, Debug)]
pub struct OutputProof {
    // The diversified base for the destination address.
    pub g_d: decaf377::Element,
    // The transmission key for the destination address.
    pub pk_d: ka::Public,
    // The clue key for the address.
    pub ck_d: fmd::ClueKey,
    // The value of the newly created note.
    pub value: Value,
    // The blinding factor used for generating the value commitment.
    pub v_blinding: Fr,
    // The blinding factor used for generating the note commitment.
    pub note_blinding: Fq,
    // The ephemeral secret key that corresponds to the public key.
    pub esk: ka::Secret,
}

impl OutputProof {
    /// Called to verify the proof using the provided public inputs.
    ///
    /// The public inputs are:
    /// * value commitment of the new note,
    /// * note commitment of the new note,
    /// * the ephemeral public key used to generate the new note.
    pub fn verify(
        &self,
        value_commitment: value::Commitment,
        note_commitment: note::Commitment,
        epk: ka::Public,
    ) -> anyhow::Result<()> {
        // Note commitment integrity.
        let s_component_transmission_key = Fq::from_bytes(self.pk_d.0);
        if let Ok(transmission_key_s) = s_component_transmission_key {
            let note_commitment_test = note::commitment(
                self.note_blinding,
                self.value,
                self.g_d,
                transmission_key_s,
                &self.ck_d,
            );

            if note_commitment != note_commitment_test {
                return Err(anyhow!("note commitment mismatch"));
            }
        } else {
            return Err(anyhow!("transmission key mismatch"));
        }

        // Value commitment integrity.
        if value_commitment != -self.value.commit(self.v_blinding) {
            return Err(anyhow!("value commitment mismatch"));
        }

        // Ephemeral public key integrity.
        if self.esk.diversified_public(&self.g_d) != epk {
            return Err(anyhow!("ephemeral public key mismatch"));
        }

        // The use of decaf means that we do not need to check that the
        // diversified basepoint is of small order. However we instead
        // check it is not identity.
        if self.g_d.is_identity() {
            return Err(anyhow!("unexpected identity"));
        }

        Ok(())
    }
}

// Conversions

impl Protobuf<transparent_proofs::SpendProof> for SpendProof {}

impl From<SpendProof> for transparent_proofs::SpendProof {
    fn from(msg: SpendProof) -> Self {
        let ak_bytes: [u8; 32] = msg.ak.into();
        let nk_bytes: [u8; 32] = msg.nk.0.to_bytes();
        let ck_d_bytes: [u8; 32] = msg.ck_d.0;
        transparent_proofs::SpendProof {
            note_commitment_proof: Some(msg.note_commitment_proof.into()),
            g_d: msg.g_d.vartime_compress().0.to_vec(),
            pk_d: msg.pk_d.0.to_vec(),
            value_amount: msg.value.amount,
            value_asset_id: msg.value.asset_id.0.to_bytes().to_vec(),
            v_blinding: msg.v_blinding.to_bytes().to_vec(),
            note_blinding: msg.note_blinding.to_bytes().to_vec(),
            spend_auth_randomizer: msg.spend_auth_randomizer.to_bytes().to_vec(),
            ak: ak_bytes.into(),
            nk: nk_bytes.into(),
            ck_d: ck_d_bytes.into(),
        }
    }
}

impl TryFrom<transparent_proofs::SpendProof> for SpendProof {
    type Error = Error;

    fn try_from(proto: transparent_proofs::SpendProof) -> anyhow::Result<Self, Self::Error> {
        let g_d_bytes: [u8; 32] = proto
            .g_d
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;
        let g_d_encoding = decaf377::Encoding(g_d_bytes);

        let ck_d_bytes: [u8; 32] = proto
            .ck_d
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;

        let v_blinding_bytes: [u8; 32] = proto.v_blinding[..]
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;

        let ak_bytes: [u8; 32] = (proto.ak[..])
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;
        let ak = ak_bytes
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;

        Ok(SpendProof {
            note_commitment_proof: proto
                .note_commitment_proof
                .ok_or_else(|| anyhow!("proto malformed"))?
                .try_into()
                .map_err(|_| anyhow!("proto malformed"))?,
            g_d: g_d_encoding
                .vartime_decompress()
                .map_err(|_| anyhow!("proto malformed"))?,
            pk_d: ka::Public(
                proto
                    .pk_d
                    .try_into()
                    .map_err(|_| anyhow!("proto malformed"))?,
            ),
            ck_d: fmd::ClueKey(ck_d_bytes),
            value: Value {
                amount: proto.value_amount,
                asset_id: asset::Id(
                    Fq::from_bytes(
                        proto
                            .value_asset_id
                            .try_into()
                            .map_err(|_| anyhow!("proto malformed"))?,
                    )
                    .map_err(|_| anyhow!("proto malformed"))?,
                ),
            },
            v_blinding: Fr::from_bytes(v_blinding_bytes).map_err(|_| anyhow!("proto malformed"))?,
            note_blinding: Fq::from_bytes(
                proto.note_blinding[..]
                    .try_into()
                    .map_err(|_| anyhow!("proto malformed"))?,
            )
            .map_err(|_| anyhow!("proto malformed"))?,
            spend_auth_randomizer: Fr::from_bytes(
                proto.spend_auth_randomizer[..]
                    .try_into()
                    .map_err(|_| anyhow!("proto malformed"))?,
            )
            .map_err(|_| anyhow!("proto malformed"))?,
            ak,
            nk: keys::NullifierKey(
                Fq::from_bytes(
                    proto.nk[..]
                        .try_into()
                        .map_err(|_| anyhow!("proto malformed"))?,
                )
                .map_err(|_| anyhow!("proto malformed"))?,
            ),
        })
    }
}

impl Protobuf<transparent_proofs::OutputProof> for OutputProof {}

impl From<OutputProof> for transparent_proofs::OutputProof {
    fn from(msg: OutputProof) -> Self {
        transparent_proofs::OutputProof {
            g_d: msg.g_d.vartime_compress().0.to_vec(),
            pk_d: msg.pk_d.0.to_vec(),
            ck_d: msg.ck_d.0.to_vec(),
            value_amount: msg.value.amount,
            value_asset_id: msg.value.asset_id.0.to_bytes().to_vec(),
            v_blinding: msg.v_blinding.to_bytes().to_vec(),
            note_blinding: msg.note_blinding.to_bytes().to_vec(),
            esk: msg.esk.to_bytes().to_vec(),
        }
    }
}

impl TryFrom<transparent_proofs::OutputProof> for OutputProof {
    type Error = Error;

    fn try_from(proto: transparent_proofs::OutputProof) -> anyhow::Result<Self, Self::Error> {
        let g_d_bytes: [u8; 32] = proto
            .g_d
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;
        let g_d_encoding = decaf377::Encoding(g_d_bytes);

        let v_blinding_bytes: [u8; 32] = proto.v_blinding[..]
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;

        let esk_bytes: [u8; 32] = proto.esk[..]
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;
        let esk = ka::Secret::new_from_field(
            Fr::from_bytes(esk_bytes).map_err(|_| anyhow!("proto malformed"))?,
        );

        let ck_bytes: [u8; 32] = proto.ck_d[..]
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;

        Ok(OutputProof {
            g_d: g_d_encoding
                .vartime_decompress()
                .map_err(|_| anyhow!("proto malformed"))?,
            pk_d: ka::Public(
                proto
                    .pk_d
                    .try_into()
                    .map_err(|_| anyhow!("proto malformed"))?,
            ),
            ck_d: fmd::ClueKey(ck_bytes),
            value: Value {
                amount: proto.value_amount,
                asset_id: asset::Id(
                    Fq::from_bytes(
                        proto
                            .value_asset_id
                            .try_into()
                            .map_err(|_| anyhow!("proto malformed"))?,
                    )
                    .map_err(|_| anyhow!("proto malformed"))?,
                ),
            },
            v_blinding: Fr::from_bytes(v_blinding_bytes).map_err(|_| anyhow!("proto malformed"))?,
            note_blinding: Fq::from_bytes(
                proto.note_blinding[..]
                    .try_into()
                    .map_err(|_| anyhow!("proto malformed"))?,
            )
            .map_err(|_| anyhow!("proto malformed"))?,
            esk,
        })
    }
}

impl From<SpendProof> for Vec<u8> {
    fn from(spend_proof: SpendProof) -> Vec<u8> {
        let protobuf_serialized_proof: transparent_proofs::SpendProof = spend_proof.into();
        protobuf_serialized_proof.encode_to_vec()
    }
}

impl TryFrom<&[u8]> for SpendProof {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<SpendProof, Self::Error> {
        let protobuf_serialized_proof = transparent_proofs::SpendProof::decode(bytes)
            .map_err(|_| anyhow!("proto malformed"))?;
        protobuf_serialized_proof
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))
    }
}

impl From<OutputProof> for Vec<u8> {
    fn from(output_proof: OutputProof) -> Vec<u8> {
        let protobuf_serialized_proof: transparent_proofs::OutputProof = output_proof.into();
        protobuf_serialized_proof.encode_to_vec()
    }
}

impl TryFrom<&[u8]> for OutputProof {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<OutputProof, Self::Error> {
        let protobuf_serialized_proof = transparent_proofs::OutputProof::decode(bytes)
            .map_err(|_| anyhow!("proto malformed"))?;
        protobuf_serialized_proof
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))
    }
}

/// Transparent proof for claiming swapped assets.
///
/// SwapClaim consumes an existing Swap NFT so they are most similar to Spend operations,
/// however the note commitment proof needs to be for a specific block due to clearing prices
/// only being valid for particular blocks (i.e. the exchange rates of assets change over time).
///
/// This structure keeps track of the auxiliary (private) inputs.
#[derive(Clone, Debug)]
pub struct SwapClaimProof {
    // Describes the input note with the Swap NFT

    // The asset ID of the swap NFT.
    pub swap_nft_asset_id: asset::Id,
    // The address associated with the swap NFT and outputs.
    pub claim_address: Address,
    // Proves the note commitment was included in the TCT.
    pub note_commitment_proof: tct::Proof,
    // The blinding factor used for generating the note commitment for the Swap NFT.
    pub note_blinding: Fq,
    // The nullifier deriving key for the Swap NFT note.
    pub nk: keys::NullifierKey,

    // Describes opening of Swap NFT asset ID for commitment verification
    pub trading_pair: TradingPair,
    pub delta_1: u64,
    pub delta_2: u64,

    // Describes output amounts
    pub lambda_1: u64,
    pub lambda_2: u64,

    // Describes first output note (lambda 1)
    pub note_blinding_1: Fq,
    pub esk_1: ka::Secret,

    // Describes second output note (lambda 2)
    pub note_blinding_2: Fq,
    pub esk_2: ka::Secret,
}

impl SwapClaimProof {
    /// Called to verify the proof using the provided public inputs.
    ///
    /// The public inputs are:
    /// * the merkle root of the note commitment tree,
    /// * value commitment of the note to be spent,
    /// * nullifier of the note to be spent,
    /// * the randomized verification spend key,
    /// * the pre-paid fee amount for the swap,
    pub fn verify(
        &self,
        anchor: tct::Root,
        nullifier: Nullifier,
        output_data: BatchSwapOutputData,
        epoch_duration: u64,
        fee: Fee,
    ) -> anyhow::Result<()> {
        // Swap NFT note commitment integrity.
        let swap_nft_value = Value {
            amount: 1,
            asset_id: self.swap_nft_asset_id,
        };
        let transmission_key_s = self.claim_address.transmission_key_s();
        let note_commitment_test = note::commitment(
            self.note_blinding,
            swap_nft_value.clone(),
            *self.claim_address.diversified_generator(),
            *transmission_key_s,
            self.claim_address.clue_key(),
        );

        if self.note_commitment_proof.commitment() != note_commitment_test {
            return Err(anyhow!("note commitment mismatch"));
        }

        // check the swap NFT Asset ID is properly constructed
        let asset_id = self.swap_nft_asset_id;
        let expected_plaintext = SwapPlaintext::from_parts(
            self.trading_pair.clone(),
            self.delta_1,
            self.delta_2,
            fee.clone(),
            // This should ensure that the claim address matches the address
            // used to construct the Swap NFT.
            self.claim_address,
        )
        .map_err(|_| anyhow!("error generating expected swap plaintext"))?;
        let expected_asset_id = expected_plaintext.asset_id();
        if expected_asset_id != asset_id {
            return Err(anyhow!("improper swap NFT asset id"));
        }

        // Merkle path integrity.
        self.note_commitment_proof
            .verify(anchor)
            .map_err(|_| anyhow!("merkle root mismatch"))?;

        // Validate the note commitment was for the proper block height.
        let position = self.note_commitment_proof.position();
        let block = position.block();
        let epoch = position.epoch();
        let note_commitment_block_height: u64 =
            epoch_duration * u64::from(epoch) + u64::from(block);
        if note_commitment_block_height != output_data.height {
            return Err(anyhow::anyhow!(
                "note commitment was not for clearing price height"
            ));
        }

        // Swap NFT nullifier integrity.
        if nullifier
            != self
                .nk
                .derive_nullifier(position, &self.note_commitment_proof.commitment())
        {
            return Err(anyhow!("bad nullifier"));
        }

        // TODO:
        // The address should be the same for the Swap NFT and SwapClaim outputs
        // Need output notes here, and to validate the amounts and addresses.
        // instructions here: https://github.com/penumbra-zone/penumbra/issues/1126
        // let lambda_1 = success.into() * (clearing_price_1 * self.delta_2)
        //     + (1 - success.into()) * self.delta_1;
        // let lambda_2 = success.into() * (clearing_price_2 * self.delta_1)
        //     + (1 - success.into()) * self.delta_2;
        // TODO: currently treating all swaps as failed, so delta == lambda
        let proof_1 = OutputProof {
            value: Value {
                amount: self.delta_1,
                asset_id: self.trading_pair.asset_1(),
            },
            // TODO: i don't think a zero blinding factor is the thing to use here, but where else would it come from
            v_blinding: Fr::zero(),
            note_blinding: self.note_blinding_1,
            esk: self.esk_1.clone(),
            g_d: *self.claim_address.diversified_generator(),
            pk_d: *self.claim_address.transmission_key(),
            ck_d: *self.claim_address.clue_key(),
        };
        // TODO: unclear how to call verify here
        // proof_1
        //     .verify()
        //     .map_err(|_| anyhow!("output proof 1 failed"))?;

        let proof_2 = OutputProof {
            value: Value {
                amount: self.delta_2,
                asset_id: self.trading_pair.asset_2(),
            },
            // TODO: i don't think a zero blinding factor is the thing to use here, but where else would it come from
            v_blinding: Fr::zero(),
            note_blinding: self.note_blinding_2,
            esk: self.esk_2.clone(),
            g_d: *self.claim_address.diversified_generator(),
            pk_d: *self.claim_address.transmission_key(),
            ck_d: *self.claim_address.clue_key(),
        };
        // TODO: unclear how to call verify here
        // proof_2
        //     .verify()
        //     .map_err(|_| anyhow!("output proof 2 failed"))?;

        Ok(())
    }
}

impl From<SwapClaimProof> for Vec<u8> {
    fn from(swap_proof: SwapClaimProof) -> Vec<u8> {
        let protobuf_serialized_proof: transparent_proofs::SwapClaimProof = swap_proof.into();
        protobuf_serialized_proof.encode_to_vec()
    }
}

impl TryFrom<&[u8]> for SwapClaimProof {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<SwapClaimProof, Self::Error> {
        let protobuf_serialized_proof = transparent_proofs::SwapClaimProof::decode(bytes)
            .map_err(|_| anyhow!("proto malformed"))?;
        protobuf_serialized_proof
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))
    }
}

impl Protobuf<transparent_proofs::SwapClaimProof> for SwapClaimProof {}

impl From<SwapClaimProof> for transparent_proofs::SwapClaimProof {
    fn from(msg: SwapClaimProof) -> Self {
        let nk_bytes: [u8; 32] = msg.nk.0.to_bytes();
        transparent_proofs::SwapClaimProof {
            note_commitment_proof: Some(msg.note_commitment_proof.into()),
            claim_address: Some(msg.claim_address.into()),
            trading_pair: Some(msg.trading_pair.into()),
            delta_1: msg.delta_1,
            delta_2: msg.delta_2,
            lambda_1: msg.lambda_1,
            lambda_2: msg.lambda_2,
            note_blinding_1: msg.note_blinding_1.to_bytes().to_vec(),
            note_blinding_2: msg.note_blinding_2.to_bytes().to_vec(),
            esk_1: msg.esk_1.to_bytes().to_vec(),
            esk_2: msg.esk_2.to_bytes().to_vec(),
            swap_nft_asset_id: msg.swap_nft_asset_id.0.to_bytes().to_vec(),
            note_blinding: msg.note_blinding.to_bytes().to_vec(),
            nk: nk_bytes.into(),
        }
    }
}

impl TryFrom<transparent_proofs::SwapClaimProof> for SwapClaimProof {
    type Error = Error;

    fn try_from(proto: transparent_proofs::SwapClaimProof) -> anyhow::Result<Self, Self::Error> {
        let esk_1_bytes: [u8; 32] = proto.esk_1[..]
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;
        let esk_1 = ka::Secret::new_from_field(
            Fr::from_bytes(esk_1_bytes).map_err(|_| anyhow!("proto malformed"))?,
        );
        let esk_2_bytes: [u8; 32] = proto.esk_2[..]
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;
        let esk_2 = ka::Secret::new_from_field(
            Fr::from_bytes(esk_2_bytes).map_err(|_| anyhow!("proto malformed"))?,
        );

        Ok(SwapClaimProof {
            esk_1,
            esk_2,
            note_blinding_1: Fq::from_le_bytes_mod_order(&proto.note_blinding_1),
            note_blinding_2: Fq::from_le_bytes_mod_order(&proto.note_blinding_2),
            lambda_2: proto.lambda_2,
            lambda_1: proto.lambda_1,
            delta_2: proto.delta_2,
            delta_1: proto.delta_1,
            trading_pair: proto
                .trading_pair
                .ok_or_else(|| anyhow!("proto malformed"))?
                .try_into()
                .map_err(|_| anyhow!("proto malformed"))?,
            note_commitment_proof: proto
                .note_commitment_proof
                .ok_or_else(|| anyhow!("proto malformed"))?
                .try_into()
                .map_err(|_| anyhow!("proto malformed"))?,
            claim_address: proto
                .claim_address
                .ok_or_else(|| anyhow!("proto malformed"))?
                .try_into()
                .map_err(|_| anyhow!("proto malformed"))?,
            swap_nft_asset_id: asset::Id(
                Fq::from_bytes(
                    proto
                        .swap_nft_asset_id
                        .try_into()
                        .map_err(|_| anyhow!("proto malformed"))?,
                )
                .map_err(|_| anyhow!("proto malformed"))?,
            ),
            note_blinding: Fq::from_bytes(
                proto.note_blinding[..]
                    .try_into()
                    .map_err(|_| anyhow!("proto malformed"))?,
            )
            .map_err(|_| anyhow!("proto malformed"))?,
            nk: keys::NullifierKey(
                Fq::from_bytes(
                    proto.nk[..]
                        .try_into()
                        .map_err(|_| anyhow!("proto malformed"))?,
                )
                .map_err(|_| anyhow!("proto malformed"))?,
            ),
        })
    }
}

/// Transparent proof for swap creation.
///
/// Swaps create an output NFT encoding the swap data so they are most similar to Output operations.
///
/// This structure keeps track of the auxiliary (private) inputs.
#[derive(Clone, Debug)]
pub struct SwapProof {
    // The address associated with the swap.
    pub claim_address: Address,
    // The value of asset 1 in the swap.
    pub value_t1: Value,
    // The value of asset 2 in the swap.
    pub value_t2: Value,
    // The fee amount associated with the swap.
    pub fee_delta: Fee,
    // The asset ID of the Swap NFT.
    pub swap_nft_asset_id: asset::Id,
    // The blinding factor used for generating the note commitment for the Swap NFT.
    pub note_blinding: Fq,
    // The ephemeral secret key that corresponds to the public key.
    pub esk: ka::Secret,
    // TODO: no value commitments for delta 1/delta 2 until flow encryption is available
    // // The blinding factor used for generating the value commitment for delta 1.
    // pub delta_1_blinding: Fr,
    // // The blinding factor used for generating the value commitment for delta 2.
    // pub delta_2_blinding: Fr,
}

impl SwapProof {
    /// Called to verify the proof using the provided public inputs.
    ///
    /// The public inputs are:
    /// * value commitment of the asset 1's contribution to the transaction,
    /// * value commitment of the asset 2's contribution to the transaction,
    /// * value commitment of the fee's contribution to the transaction,
    /// * note commitment of the new swap NFT note,
    /// * the ephemeral public key used to generate the new swap NFT note.
    pub fn verify(
        &self,
        _value_1_commitment: value::Commitment,
        _value_2_commitment: value::Commitment,
        value_fee_commitment: value::Commitment,
        note_commitment: note::Commitment,
        epk: ka::Public,
    ) -> anyhow::Result<(), Error> {
        // Note commitment integrity.
        let transmission_key_s = self.claim_address.transmission_key_s();
        // Checks the note commitment of the Swap NFT.
        let note_commitment_test = note::commitment(
            self.note_blinding,
            Value {
                // The swap NFT is always amount 1.
                amount: 1,
                asset_id: self.swap_nft_asset_id,
            },
            *self.claim_address.diversified_generator(),
            *transmission_key_s,
            &self.claim_address.clue_key(),
        );

        if note_commitment != note_commitment_test {
            return Err(anyhow!("note commitment mismatch"));
        }

        // TODO: no value commitment checks until flow encryption is available
        // // Value commitment integrity.
        // if value_1_commitment != -self.value_t1.commit(self.delta_1_blinding) {
        //     return Err(anyhow!("value commitment mismatch"));
        // }

        // if value_2_commitment != -self.value_t2.commit(self.delta_2_blinding) {
        //     return Err(anyhow!("value commitment mismatch"));
        // }

        let fee_blinding = Fr::zero();
        if value_fee_commitment != -self.fee_delta.commit(fee_blinding) {
            return Err(anyhow!("value commitment mismatch"));
        }

        // Ephemeral public key integrity.
        if self
            .esk
            .diversified_public(self.claim_address.diversified_generator())
            != epk
        {
            return Err(anyhow!("ephemeral public key mismatch"));
        }

        // The use of decaf means that we do not need to check that the
        // diversified basepoint is of small order. However we instead
        // check it is not identity.
        if self.claim_address.diversified_generator().is_identity() {
            return Err(anyhow!("unexpected identity"));
        }

        Ok(())
    }
}

impl Protobuf<transparent_proofs::SwapProof> for SwapProof {}

impl From<SwapProof> for transparent_proofs::SwapProof {
    fn from(msg: SwapProof) -> Self {
        transparent_proofs::SwapProof {
            claim_address: Some(msg.claim_address.into()),
            delta_1: msg.value_t1.amount,
            t1: msg.value_t1.asset_id.0.to_bytes().to_vec(),
            delta_2: msg.value_t2.amount,
            t2: msg.value_t2.asset_id.0.to_bytes().to_vec(),
            fee: Some(msg.fee_delta.into()),
            swap_nft_asset_id: msg.swap_nft_asset_id.0.to_bytes().to_vec(),
            // TODO: no value commitments for delta 1/delta 2 until flow encryption is available
            // delta_1_blinding: msg.delta_1_blinding.to_bytes().to_vec(),
            // delta_2_blinding: msg.delta_2_blinding.to_bytes().to_vec(),
            note_blinding: msg.note_blinding.to_bytes().to_vec(),
            esk: msg.esk.to_bytes().to_vec(),
        }
    }
}

impl TryFrom<transparent_proofs::SwapProof> for SwapProof {
    type Error = Error;

    fn try_from(proto: transparent_proofs::SwapProof) -> anyhow::Result<Self, Self::Error> {
        // let delta_1_blinding_bytes: [u8; 32] = proto.delta_1_blinding[..]
        //     .try_into()
        //     .map_err(|_| anyhow!("proto malformed"))?;
        // let delta_2_blinding_bytes: [u8; 32] = proto.delta_2_blinding[..]
        //     .try_into()
        //     .map_err(|_| anyhow!("proto malformed"))?;

        let esk_bytes: [u8; 32] = proto.esk[..]
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))?;
        let esk = ka::Secret::new_from_field(
            Fr::from_bytes(esk_bytes).map_err(|_| anyhow!("proto malformed"))?,
        );

        let _pen_denom = asset::REGISTRY.parse_denom("upenumbra").unwrap();

        Ok(SwapProof {
            claim_address: proto
                .claim_address
                .ok_or(anyhow!("proto malformed"))?
                .try_into()
                .map_err(|_| anyhow!("proto malformed"))?,
            value_t1: Value {
                amount: proto.delta_1,
                asset_id: asset::Id(
                    Fq::from_bytes(
                        proto
                            .t1
                            .try_into()
                            .map_err(|_| anyhow!("proto malformed"))?,
                    )
                    .map_err(|_| anyhow!("proto malformed"))?,
                ),
            },
            value_t2: Value {
                amount: proto.delta_2,
                asset_id: asset::Id(
                    Fq::from_bytes(
                        proto
                            .t2
                            .try_into()
                            .map_err(|_| anyhow!("proto malformed"))?,
                    )
                    .map_err(|_| anyhow!("proto malformed"))?,
                ),
            },
            fee_delta: proto
                .fee
                .ok_or_else(|| anyhow::anyhow!("proto malformed"))?
                .try_into()
                .map_err(|_| anyhow!("proto malformed"))?,
            swap_nft_asset_id: asset::Id(
                Fq::from_bytes(
                    proto
                        .swap_nft_asset_id
                        .try_into()
                        .map_err(|_| anyhow!("proto malformed"))?,
                )
                .map_err(|_| anyhow!("proto malformed"))?,
            ),
            // TODO: no value commitment checks until flow encryption is available
            // delta_1_blinding: Fr::from_bytes(delta_1_blinding_bytes)
            //     .map_err(|_| anyhow!("proto malformed"))?,
            // delta_2_blinding: Fr::from_bytes(delta_2_blinding_bytes)
            //     .map_err(|_| anyhow!("proto malformed"))?,
            note_blinding: Fq::from_bytes(
                proto.note_blinding[..]
                    .try_into()
                    .map_err(|_| anyhow!("proto malformed"))?,
            )
            .map_err(|_| anyhow!("proto malformed"))?,
            esk,
        })
    }
}

impl From<SwapProof> for Vec<u8> {
    fn from(output_proof: SwapProof) -> Vec<u8> {
        let protobuf_serialized_proof: transparent_proofs::SwapProof = output_proof.into();
        protobuf_serialized_proof.encode_to_vec()
    }
}

impl TryFrom<&[u8]> for SwapProof {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<SwapProof, Self::Error> {
        let protobuf_serialized_proof =
            transparent_proofs::SwapProof::decode(bytes).map_err(|_| anyhow!("proto malformed"))?;
        protobuf_serialized_proof
            .try_into()
            .map_err(|_| anyhow!("proto malformed"))
    }
}

#[cfg(test)]
mod tests {
    use ark_ff::UniformRand;
    use rand_core::OsRng;

    use super::*;
    use crate::{
        keys::{SeedPhrase, SpendKey},
        note, Note, Value,
    };

    #[test]
    fn test_output_proof_verification_success() {
        let mut rng = OsRng;

        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_recipient = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_recipient = sk_recipient.full_viewing_key();
        let ivk_recipient = fvk_recipient.incoming();
        let (dest, _dtk_d) = ivk_recipient.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);
        let note = Note::generate(&mut rng, &dest, value_to_send);
        let esk = ka::Secret::new(&mut rng);
        let epk = esk.diversified_public(&note.diversified_generator());

        let proof = OutputProof {
            g_d: *dest.diversified_generator(),
            pk_d: *dest.transmission_key(),
            ck_d: *dest.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            esk,
        };

        assert!(proof
            .verify(-value_to_send.commit(v_blinding), note.commit(), epk)
            .is_ok());
    }

    #[test]
    fn test_output_proof_verification_note_commitment_integrity_failure() {
        let mut rng = OsRng;

        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_recipient = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_recipient = sk_recipient.full_viewing_key();
        let ivk_recipient = fvk_recipient.incoming();
        let (dest, _dtk_d) = ivk_recipient.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);
        let note = Note::generate(&mut rng, &dest, value_to_send);
        let esk = ka::Secret::new(&mut rng);
        let epk = esk.diversified_public(&note.diversified_generator());

        let proof = OutputProof {
            g_d: *dest.diversified_generator(),
            pk_d: *dest.transmission_key(),
            ck_d: *dest.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            esk,
        };

        let incorrect_note_commitment = note::commitment(
            Fq::rand(&mut rng),
            value_to_send,
            note.diversified_generator(),
            note.transmission_key_s(),
            note.clue_key(),
        );

        assert!(proof
            .verify(
                -value_to_send.commit(v_blinding),
                incorrect_note_commitment,
                epk
            )
            .is_err());
    }

    #[test]
    fn test_output_proof_verification_value_commitment_integrity_failure() {
        let mut rng = OsRng;

        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_recipient = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_recipient = sk_recipient.full_viewing_key();
        let ivk_recipient = fvk_recipient.incoming();
        let (dest, _dtk_d) = ivk_recipient.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);
        let note = Note::generate(&mut rng, &dest, value_to_send);
        let esk = ka::Secret::new(&mut rng);
        let correct_epk = esk.diversified_public(&note.diversified_generator());

        let proof = OutputProof {
            g_d: *dest.diversified_generator(),
            pk_d: *dest.transmission_key(),
            ck_d: *dest.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            esk,
        };
        let incorrect_value_commitment = value_to_send.commit(Fr::rand(&mut rng));

        assert!(proof
            .verify(incorrect_value_commitment, note.commit(), correct_epk)
            .is_err());
    }

    #[test]
    fn test_output_proof_verification_ephemeral_public_key_integrity_failure() {
        let mut rng = OsRng;

        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_recipient = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_recipient = sk_recipient.full_viewing_key();
        let ivk_recipient = fvk_recipient.incoming();
        let (dest, _dtk_d) = ivk_recipient.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);
        let note = Note::generate(&mut rng, &dest, value_to_send);
        let esk = ka::Secret::new(&mut rng);

        let proof = OutputProof {
            g_d: *dest.diversified_generator(),
            pk_d: *dest.transmission_key(),
            ck_d: *dest.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            esk,
        };
        let incorrect_esk = ka::Secret::new(&mut rng);
        let incorrect_epk = incorrect_esk.diversified_public(&note.diversified_generator());

        assert!(proof
            .verify(
                -value_to_send.commit(v_blinding),
                note.commit(),
                incorrect_epk
            )
            .is_err());
    }

    #[test]
    fn test_output_proof_verification_identity_check_failure() {
        let mut rng = OsRng;

        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_recipient = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_recipient = sk_recipient.full_viewing_key();
        let ivk_recipient = fvk_recipient.incoming();
        let (dest, _dtk_d) = ivk_recipient.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);
        let note = Note::generate(&mut rng, &dest, value_to_send);
        let esk = ka::Secret::new(&mut rng);
        let epk = esk.diversified_public(&note.diversified_generator());

        let proof = OutputProof {
            g_d: decaf377::Element::default(),
            pk_d: *dest.transmission_key(),
            ck_d: *dest.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            esk,
        };

        assert!(proof
            .verify(-value_to_send.commit(v_blinding), note.commit(), epk)
            .is_err());
    }

    #[test]
    fn test_spend_proof_verification_success() {
        let mut rng = OsRng;

        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_sender = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_sender = sk_sender.full_viewing_key();
        let ivk_sender = fvk_sender.incoming();
        let (sender, _dtk_d) = ivk_sender.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);

        let note = Note::generate(&mut rng, &sender, value_to_send);
        let note_commitment = note.commit();
        let spend_auth_randomizer = Fr::rand(&mut rng);
        let rsk = sk_sender.spend_auth_key().randomize(&spend_auth_randomizer);
        let nk = *sk_sender.nullifier_key();
        let ak = sk_sender.spend_auth_key().into();
        let mut nct = tct::Tree::new();
        nct.insert(tct::Witness::Keep, note_commitment).unwrap();
        let anchor = nct.root();
        let note_commitment_proof = nct.witness(note_commitment).unwrap();

        let proof = SpendProof {
            note_commitment_proof,
            g_d: *sender.diversified_generator(),
            pk_d: *sender.transmission_key(),
            ck_d: *sender.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            spend_auth_randomizer,
            ak,
            nk,
        };

        let rk: VerificationKey<SpendAuth> = rsk.into();
        let nf = nk.derive_nullifier(0.into(), &note_commitment);
        assert!(proof
            .verify(anchor, value_to_send.commit(v_blinding), nf, rk)
            .is_ok());
    }

    #[test]
    fn test_spend_proof_verification_merkle_path_integrity_failure() {
        let mut rng = OsRng;
        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_sender = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_sender = sk_sender.full_viewing_key();
        let ivk_sender = fvk_sender.incoming();
        let (sender, _dtk_d) = ivk_sender.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);

        let note = Note::generate(&mut rng, &sender, value_to_send);
        let note_commitment = note.commit();
        let spend_auth_randomizer = Fr::rand(&mut rng);
        let rsk = sk_sender.spend_auth_key().randomize(&spend_auth_randomizer);
        let nk = *sk_sender.nullifier_key();
        let ak = sk_sender.spend_auth_key().into();
        let mut nct = tct::Tree::new();
        let incorrect_anchor = nct.root();
        nct.insert(tct::Witness::Keep, note_commitment).unwrap();
        let note_commitment_proof = nct.witness(note_commitment).unwrap();

        let proof = SpendProof {
            note_commitment_proof,
            g_d: *sender.diversified_generator(),
            pk_d: *sender.transmission_key(),
            ck_d: *sender.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            spend_auth_randomizer,
            ak,
            nk,
        };

        let rk: VerificationKey<SpendAuth> = rsk.into();
        let nf = nk.derive_nullifier(0.into(), &note_commitment);
        assert!(proof
            .verify(incorrect_anchor, value_to_send.commit(v_blinding), nf, rk)
            .is_err());
    }

    #[test]
    fn test_spend_proof_verification_value_commitment_integrity_failure() {
        let mut rng = OsRng;
        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_sender = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_sender = sk_sender.full_viewing_key();
        let ivk_sender = fvk_sender.incoming();
        let (sender, _dtk_d) = ivk_sender.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);
        let note = Note::generate(&mut rng, &sender, value_to_send);
        let note_commitment = note.commit();
        let spend_auth_randomizer = Fr::rand(&mut rng);
        let rsk = sk_sender.spend_auth_key().randomize(&spend_auth_randomizer);
        let nk = *sk_sender.nullifier_key();
        let ak = sk_sender.spend_auth_key().into();
        let mut nct = tct::Tree::new();
        nct.insert(tct::Witness::Keep, note_commitment).unwrap();
        let anchor = nct.root();
        let note_commitment_proof = nct.witness(note_commitment).unwrap();

        let proof = SpendProof {
            note_commitment_proof,
            g_d: *sender.diversified_generator(),
            pk_d: *sender.transmission_key(),
            ck_d: *sender.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            spend_auth_randomizer,
            ak,
            nk,
        };

        let rk: VerificationKey<SpendAuth> = rsk.into();
        let nf = nk.derive_nullifier(0.into(), &note_commitment);
        assert!(proof
            .verify(anchor, value_to_send.commit(Fr::rand(&mut rng)), nf, rk)
            .is_err());
    }

    #[test]
    fn test_spend_proof_verification_nullifier_integrity_failure() {
        let mut rng = OsRng;
        let seed_phrase = SeedPhrase::generate(&mut rng);
        let sk_sender = SpendKey::from_seed_phrase(seed_phrase, 0);
        let fvk_sender = sk_sender.full_viewing_key();
        let ivk_sender = fvk_sender.incoming();
        let (sender, _dtk_d) = ivk_sender.payment_address(0u64.into());

        let value_to_send = Value {
            amount: 10,
            asset_id: asset::REGISTRY.parse_denom("upenumbra").unwrap().id(),
        };
        let v_blinding = Fr::rand(&mut rng);
        let note = Note::generate(&mut rng, &sender, value_to_send);
        let note_commitment = note.commit();
        let spend_auth_randomizer = Fr::rand(&mut rng);
        let rsk = sk_sender.spend_auth_key().randomize(&spend_auth_randomizer);
        let nk = *sk_sender.nullifier_key();
        let ak = sk_sender.spend_auth_key().into();
        let mut nct = tct::Tree::new();
        nct.insert(tct::Witness::Keep, note_commitment).unwrap();
        let anchor = nct.root();
        let note_commitment_proof = nct.witness(note_commitment).unwrap();

        let proof = SpendProof {
            note_commitment_proof,
            g_d: *sender.diversified_generator(),
            pk_d: *sender.transmission_key(),
            ck_d: *sender.clue_key(),
            value: value_to_send,
            v_blinding,
            note_blinding: note.note_blinding(),
            spend_auth_randomizer,
            ak,
            nk,
        };

        let rk: VerificationKey<SpendAuth> = rsk.into();
        let incorrect_nf = nk.derive_nullifier(5.into(), &note_commitment);
        assert!(proof
            .verify(anchor, value_to_send.commit(v_blinding), incorrect_nf, rk)
            .is_err());
    }
}
