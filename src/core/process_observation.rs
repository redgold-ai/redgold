use dashmap::mapref::one::Ref;
use futures::{StreamExt, TryStreamExt};
use log::{debug, info};
use metrics::counter;
use redgold_schema::structs::{ErrorInfo, Hash, HashType, Observation, Transaction};
use redgold_schema::{util, WithMetadataHashable};
use crate::core::internal_message::RecvAsyncErrorInfo;
use crate::core::relay::Relay;
use redgold_schema::EasyJson;
use crate::core::process_transaction::{ProcessTransactionMessage, RequestProcessor};

#[derive(Clone)]
pub struct ObservationHandler {
    pub relay: Relay,
}

impl ObservationHandler {
    async fn notify_subscribers(&self, o: &Transaction) {
        // Notify subscribers
        // TODO: FP
        let _h = o.hash_or();
        if let Ok(proofs) = o.build_observation_proofs() {
            for proof in proofs {
                if let Some(m) = &proof.metadata {
                    for hash in &m.observed_hash {
                        if hash.hash_type == HashType::Transaction as i32 {
                            if let Some(r) = self.relay.transaction_channels.get(&hash) {
                                let message = ProcessTransactionMessage::ProofReceived(proof.clone());
                                r.internal_channel.sender.try_send(message)
                                    .unwrap_or_else(|e| {
                                        tracing::error!("Failed to send proof received message to transaction processor: {}", e);
                                        counter!("redgold.observation.failed_to_send_to_transaction_processor").increment(1);
                                    });
                            }
                        }
                    }
                }
            }
        }
    }

    async fn process_message(&self, o: Transaction) -> Result<(), ErrorInfo> {
        counter!("redgold.observation.received").increment(1);
        debug!("Received peer observation {}", o.json_or());
        // TODO: Verify merkle root
        // TODO: Verify time and/or avoid updating time if row already present.
        // TODO: Verify there is no conflicting data to our knowledge in the observation,
        // I.e. no obvious rejections, not a complete validation but partial
        // Distinguish if we have validated this entirely before observing, some we will be
        // able to.

        self.notify_subscribers(&o).await;
        self.relay.ds.observation.insert_observation_and_edges(&o).await?;

        Ok(())
    }

    // TODO: Pass in the dependencies directly.
    pub async fn run(&self) -> Result<(), ErrorInfo> {
        let receiver = self.relay.observation.receiver.clone();
        receiver.into_stream().map(Ok).try_for_each_concurrent(
            200, |o| {
                let s = self.clone();
                async move {
                    s.process_message(o).await
                }
            }).await
    }
}