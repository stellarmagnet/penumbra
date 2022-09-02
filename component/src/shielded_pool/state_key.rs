use penumbra_crypto::{asset, note, Nullifier};
use penumbra_tct::{
    builder::{block, epoch},
    Root,
};
use std::string::String;

pub fn token_supply(asset_id: &asset::Id) -> String {
    format!("shielded_pool/assets/{}/token_supply", asset_id)
}

pub fn known_assets() -> &'static str {
    "shielded_pool/known_assets"
}

pub fn denom_by_asset(asset_id: &asset::Id) -> String {
    format!("shielded_pool/assets/{}/denom", asset_id)
}

pub fn note_source(note_commitment: note::Commitment) -> String {
    format!("shielded_pool/note_source/{}", note_commitment)
}

pub fn compact_block(height: u64) -> String {
    format!("shielded_pool/compact_block/{}", height)
}

pub fn anchor_by_height(height: u64) -> String {
    format!("shielded_pool/anchor/{}", height)
}

pub fn anchor_lookup(anchor: Root) -> String {
    format!("shielded_pool/valid_anchors/{}", anchor)
}

pub fn epoch_anchor_by_index(index: u64) -> String {
    format!("shielded_pool/epoch_anchor/{}", index)
}

pub fn epoch_anchor_lookup(anchor: epoch::Root) -> String {
    format!("shielded_pool/valid_epoch_anchors/{}", anchor)
}

pub fn block_anchor_by_height(height: u64) -> String {
    format!("shielded_pool/block_anchor/{}", height)
}

pub fn block_anchor_lookup(anchor: block::Root) -> String {
    format!("shielded_pool/valid_block_anchors/{}", anchor)
}

pub fn spent_nullifier_lookup(nullifier: Nullifier) -> String {
    format!("shielded_pool/spent_nullifiers/{}", nullifier)
}

pub fn commission_amounts(height: u64) -> String {
    format!("staking/commission_amounts/{}", height)
}

pub fn claimed_swap_outputs(height: u64) -> String {
    format!("dex/claimed_swap_outputs/{}", height)
}

pub fn scheduled_to_apply(epoch: u64) -> String {
    format!("shielded_pool/quarantined_to_apply_in_epoch/{}", epoch)
}

pub fn quarantined_spent_nullifier_lookup(nullifier: Nullifier) -> String {
    format!("shielded_pool/quarantined_spent_nullifiers/{}", nullifier)
}

pub use crate::stake::state_key::slashed_validators;
