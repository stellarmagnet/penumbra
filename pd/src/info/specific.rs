use penumbra_chain::View as _;
use penumbra_component::dex::View as _;
use penumbra_component::shielded_pool::View as _;
use penumbra_component::stake::View as _;
use penumbra_proto::{
    self as proto,
    chain::NoteSource,
    client::specific::{
        specific_query_server::SpecificQuery, KeyValueRequest, KeyValueResponse,
        ValidatorStatusRequest,
    },
    crypto::NoteCommitment,
    dex::BatchSwapOutputData,
};

use proto::client::specific::BatchSwapOutputDataRequest;
use tonic::Status;
use tracing::instrument;

// We need to use the tracing-futures version of Instrument,
// because we want to instrument a Stream, and the Stream trait
// isn't in std, and the tracing::Instrument trait only works with
// (stable) std types.
//use tracing_futures::Instrument;

use super::Info;

#[tonic::async_trait]
impl SpecificQuery for Info {
    #[instrument(skip(self, request))]
    async fn transaction_by_note(
        &self,
        request: tonic::Request<NoteCommitment>,
    ) -> Result<tonic::Response<NoteSource>, Status> {
        let state = self.state_tonic().await?;
        let cm = request
            .into_inner()
            .try_into()
            .map_err(|_| Status::invalid_argument("invalid commitment"))?;
        let source = state
            .note_source(cm)
            .await
            .map_err(|e| Status::unavailable(format!("error getting note source: {}", e)))?
            .ok_or_else(|| Status::not_found("note commitment not found"))?;
        tracing::debug!(?cm, ?source);

        Ok(tonic::Response::new(source.into()))
    }

    #[instrument(skip(self, request))]
    async fn validator_status(
        &self,
        request: tonic::Request<ValidatorStatusRequest>,
    ) -> Result<tonic::Response<proto::stake::ValidatorStatus>, Status> {
        let state = self.state_tonic().await?;
        state.check_chain_id(&request.get_ref().chain_id).await?;

        let id = request
            .into_inner()
            .identity_key
            .ok_or_else(|| Status::invalid_argument("missing identity key"))?
            .try_into()
            .map_err(|_| Status::invalid_argument("invalid identity key"))?;

        let status = state
            .validator_status(&id)
            .await
            .map_err(|e| Status::unavailable(format!("error getting validator status: {}", e)))?
            .ok_or_else(|| Status::not_found("validator not found"))?;

        Ok(tonic::Response::new(status.into()))
    }

    #[instrument(skip(self, request))]
    /// Get the batch swap data associated with a given trading pair and height.
    async fn batch_swap_output_data(
        &self,
        request: tonic::Request<BatchSwapOutputDataRequest>,
    ) -> Result<tonic::Response<BatchSwapOutputData>, Status> {
        let state = self.state_tonic().await?;
        let request_inner = request.into_inner();
        let height = request_inner.height;
        let trading_pair = request_inner
            .trading_pair
            .ok_or_else(|| Status::invalid_argument("missing trading_pair"))?
            .try_into()
            .map_err(|_| Status::invalid_argument("invalid trading_pair"))?;

        let output_data = state
            .output_data(height, trading_pair)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        match output_data {
            Some(o) => Ok(tonic::Response::new(o.into())),
            None => Err(Status::not_found("batch swap output data not found")),
        }
    }

    #[instrument(skip(self, request))]
    async fn next_validator_rate(
        &self,
        request: tonic::Request<proto::crypto::IdentityKey>,
    ) -> Result<tonic::Response<proto::stake::RateData>, Status> {
        let state = self.state_tonic().await?;
        let identity_key = request
            .into_inner()
            .try_into()
            .map_err(|_| tonic::Status::invalid_argument("invalid identity key"))?;

        let rate_data = state
            .next_validator_rate(&identity_key)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        match rate_data {
            Some(r) => Ok(tonic::Response::new(r.into())),
            None => Err(Status::not_found("next validator rate not found")),
        }
    }

    #[instrument(skip(self, request))]
    async fn key_value(
        &self,
        request: tonic::Request<KeyValueRequest>,
    ) -> Result<tonic::Response<KeyValueResponse>, Status> {
        let state = self.state_tonic().await?;
        state.check_chain_id(&request.get_ref().chain_id).await?;

        let request = request.into_inner();
        tracing::debug!(?request);

        if request.key.is_empty() {
            return Err(Status::invalid_argument("key is empty"));
        }

        let (value, proof) = state
            .read()
            .await
            .get_with_proof(request.key)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let commitment_proof = ics23::CommitmentProof {
            proof: Some(ics23::commitment_proof::Proof::Exist(proof)),
        };

        Ok(tonic::Response::new(KeyValueResponse {
            value,
            proof: if request.proof {
                Some(commitment_proof)
            } else {
                None
            },
        }))
    }
}
