use crate::{
    core::{client::height::Height, connection::version::Version},
    CosmosAccountId, IntoProto, MsgIntoProto,
};

pub struct MsgConnectionOpenAck<ClientState> {
    pub connection_id: String,
    pub counterparty_connection_id: String,
    pub version: Version,
    pub client_state: ClientState,
    pub proof_height: Height,
    pub proof_try: Vec<u8>,
    pub proof_client: Vec<u8>,
    pub proof_consensus: Vec<u8>,
    pub consensus_height: Height,
}

impl<ClientState> MsgIntoProto for MsgConnectionOpenAck<ClientState>
where
    ClientState: IntoProto<Proto = protos::google::protobuf::Any>,
    // <ClientState as IntoProto>::Proto: TypeUrl,
{
    type Proto = protos::ibc::core::connection::v1::MsgConnectionOpenAck;

    fn into_proto_with_signer(self, signer: &CosmosAccountId) -> Self::Proto {
        Self::Proto {
            connection_id: self.connection_id,
            counterparty_connection_id: self.counterparty_connection_id,
            version: Some(self.version.into()),
            client_state: Some(self.client_state.into_proto()),
            proof_height: Some(self.proof_height.into()),
            proof_try: self.proof_try,
            proof_client: self.proof_client,
            proof_consensus: self.proof_consensus,
            consensus_height: Some(self.consensus_height.into()),
            signer: signer.to_string(),
            host_consensus_state_proof: vec![],
        }
    }
}

impl<ClientState> From<MsgConnectionOpenAck<ClientState>>
    for contracts::ibc_handler::MsgConnectionOpenAck
{
    fn from(msg: MsgConnectionOpenAck<ClientState>) -> Self {
        Self {
            connection_id: msg.connection_id,
            counterparty_connection_id: msg.counterparty_connection_id,
            version: msg.version.into(),
            // client_state_bytes: msg.client_state.value.into(),
            // TODO(benluelo): Figure out what this is expected to be (i.e. eth abi or proto)
            client_state_bytes: Default::default(),
            proof_height: msg.proof_height.into(),
            proof_try: msg.proof_try.into(),
            proof_client: msg.proof_client.into(),
            proof_consensus: msg.proof_consensus.into(),
            consensus_height: msg.consensus_height.into(),
        }
    }
}