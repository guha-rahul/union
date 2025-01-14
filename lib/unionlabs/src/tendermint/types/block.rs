use macros::model;

use crate::{
    errors::{required, MissingField},
    tendermint::types::{
        commit::{Commit, TryFromCommitError},
        data::Data,
        evidence_list::{EvidenceList, TryFromEvidenceListError},
        header::{Header, TryFromHeaderError},
    },
};

#[model(proto(raw(protos::tendermint::types::Block), from, into))]
pub struct Block {
    pub header: Header,
    pub data: Data,
    pub evidence: EvidenceList,
    pub last_commit: Commit,
}

impl From<Block> for protos::tendermint::types::Block {
    fn from(value: Block) -> Self {
        Self {
            header: Some(value.header.into()),
            data: Some(value.data.into()),
            evidence: Some(value.evidence.into()),
            last_commit: Some(value.last_commit.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TryFromBlockError {
    #[error(transparent)]
    MissingField(#[from] MissingField),
    #[error("invalid header")]
    Header(#[from] TryFromHeaderError),
    #[error("invalid evidence list")]
    EvidenceList(#[from] TryFromEvidenceListError),
    #[error("invalid commit")]
    Commit(#[from] TryFromCommitError),
}

impl TryFrom<protos::tendermint::types::Block> for Block {
    type Error = TryFromBlockError;

    fn try_from(value: protos::tendermint::types::Block) -> Result<Self, Self::Error> {
        Ok(Self {
            header: required!(value.header)?.try_into()?,
            data: required!(value.data)?.into(),
            evidence: required!(value.evidence)?.try_into()?,
            last_commit: required!(value.last_commit)?.try_into()?,
        })
    }
}
