use std::sync::Arc;

use async_trait::async_trait;
use prost::DecodeError;

pub use actor::Actor;
pub use aggregation::*;
pub use dispatch::*;
pub use filter::*;
pub use hash_agg::*;
pub use kafka_source::*;
pub use merge::*;
pub use mview_sink::*;
pub use project::*;
use risingwave_common::error::Result;
use risingwave_common::error::{ErrorCode, RwError};
use risingwave_pb::data::Op as ProstOp;
use risingwave_pb::data::{
    stream_message::StreamMessage, Barrier, StreamChunk as ProstStreamChunk,
    StreamMessage as ProstStreamMessage, Terminate,
};
use risingwave_pb::ToProst;
use risingwave_pb::ToProto;
pub use simple_agg::*;
pub use table_source::*;

use risingwave_common::array::column::Column;
use risingwave_common::array::DataChunk;
use risingwave_common::buffer::Bitmap;
use risingwave_common::catalog::Schema;

mod actor;
mod aggregation;
mod dispatch;
mod filter;
mod hash_agg;
mod kafka_source;
mod merge;
mod mview_sink;
mod project;
mod simple_agg;
mod table_source;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod test_utils;

pub trait ExprFn = Fn(&DataChunk) -> Result<Bitmap> + Send + Sync + 'static;

/// `Op` represents three operations in `StreamChunk`.
/// `UpdateDelete` and `UpdateInsert` always appear in pairs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Insert,
    Delete,
    UpdateDelete,
    UpdateInsert,
}

impl Op {
    pub fn to_protobuf(self) -> ProstOp {
        match self {
            Op::Insert => ProstOp::Insert,
            Op::Delete => ProstOp::Delete,
            Op::UpdateInsert => ProstOp::UpdateInsert,
            Op::UpdateDelete => ProstOp::UpdateDelete,
        }
    }

    pub fn from_protobuf(prost: &i32) -> Result<Op> {
        let op = match ProstOp::from_i32(*prost) {
            Some(ProstOp::Insert) => Op::Insert,
            Some(ProstOp::Delete) => Op::Delete,
            Some(ProstOp::UpdateInsert) => Op::UpdateInsert,
            Some(ProstOp::UpdateDelete) => Op::UpdateDelete,
            None => {
                return Err(RwError::from(ErrorCode::ProstError(DecodeError::new(
                    "No such op type",
                ))))
            }
        };
        Ok(op)
    }
}

pub type Ops<'a> = &'a [Op];

/// `StreamChunk` is used to pass data between executors.
#[derive(Default, Debug, Clone)]
pub struct StreamChunk {
    // TODO: Optimize using bitmap
    ops: Vec<Op>,
    columns: Vec<Column>,
    visibility: Option<Bitmap>,
}

impl StreamChunk {
    pub fn new(ops: Vec<Op>, columns: Vec<Column>, visibility: Option<Bitmap>) -> Self {
        StreamChunk {
            ops,
            columns,
            visibility,
        }
    }

    /// return the number of visible tuples
    pub fn cardinality(&self) -> usize {
        if let Some(bitmap) = &self.visibility {
            bitmap.iter().map(|visible| visible as usize).sum()
        } else {
            self.capacity()
        }
    }

    /// return physical length of any chunk column
    pub fn capacity(&self) -> usize {
        self.columns
            .first()
            .map(|col| col.array_ref().len())
            .unwrap_or(0)
    }

    /// compact the `StreamChunck` with its visibility map
    pub fn compact(self) -> Result<Self> {
        match &self.visibility {
            None => Ok(self),
            Some(visibility) => {
                let cardinality = visibility
                    .iter()
                    .fold(0, |vis_cnt, vis| vis_cnt + vis as usize);
                let columns = self
                    .columns
                    .into_iter()
                    .map(|col| {
                        let array = col.array();
                        let data_type = col.data_type();
                        array
                            .compact(visibility, cardinality)
                            .map(|array| Column::new(Arc::new(array), data_type))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut ops = Vec::with_capacity(cardinality);
                for (op, visible) in self.ops.into_iter().zip(visibility.iter()) {
                    if visible {
                        ops.push(op);
                    }
                }
                Ok(StreamChunk {
                    ops,
                    columns,
                    visibility: None,
                })
            }
        }
    }

    pub fn to_protobuf(&self) -> Result<ProstStreamChunk> {
        Ok(ProstStreamChunk {
            cardinality: self.cardinality() as u32,
            ops: self.ops.iter().map(|op| op.to_protobuf() as i32).collect(),
            columns: self
                .columns
                .iter()
                .map(|col| Ok(col.to_protobuf()?.to_prost::<risingwave_pb::data::Column>()))
                .collect::<Result<Vec<_>>>()?,
        })
    }

    pub fn from_protobuf(prost: &ProstStreamChunk) -> Result<Self> {
        let cardinality = prost.get_cardinality() as usize;
        let mut stream_chunk = StreamChunk {
            ops: vec![],
            columns: vec![],
            visibility: None,
        };
        for op in prost.get_ops() {
            stream_chunk.ops.push(Op::from_protobuf(op)?);
        }

        for column in prost.get_columns() {
            let proto_column = column.to_proto::<risingwave_proto::data::Column>();
            stream_chunk
                .columns
                .push(Column::from_protobuf(proto_column, cardinality)?);
        }

        Ok(stream_chunk)
    }
}

#[derive(Debug)]
pub enum Message {
    Chunk(StreamChunk),
    Barrier { epoch: u64, stop: bool },
    // Note(eric): consider remove this. A stream is always terminated by an error or dropped by user
    Terminate,
    // TODO: Watermark
}

impl Message {
    pub fn to_protobuf(&self) -> Result<StreamMessage> {
        let prost = match self {
            Self::Chunk(stream_chunk) => {
                let prost_stream_chunk = stream_chunk.to_protobuf()?;
                StreamMessage::StreamChunk(prost_stream_chunk)
            }
            Self::Barrier { epoch, stop } => StreamMessage::Barrier(Barrier {
                epoch: *epoch,
                stop: *stop,
            }),
            Self::Terminate => StreamMessage::Terminate(Terminate {}),
        };
        Ok(prost)
    }

    pub fn from_protobuf(prost: ProstStreamMessage) -> Result<Self> {
        let res = match prost.get_stream_message() {
            StreamMessage::StreamChunk(stream_chunk) => {
                Message::Chunk(StreamChunk::from_protobuf(stream_chunk)?)
            }
            StreamMessage::Barrier(epoch) => Message::Barrier {
                epoch: epoch.get_epoch(),
                stop: false,
            },
            StreamMessage::Terminate(..) => Message::Terminate,
        };
        Ok(res)
    }
}

/// `Executor` supports handling of control messages.
#[async_trait]
pub trait Executor: Send + Sync + 'static {
    async fn next(&mut self) -> Result<Message>;

    /// Return the schema of the executor.
    fn schema(&self) -> &Schema {
        todo!("A placeholder now")
    }
}

/// `SimpleExecutor` accepts a single chunk as input.
pub trait SimpleExecutor: Executor {
    fn consume_chunk(&mut self, chunk: StreamChunk) -> Result<Message>;
    fn input(&mut self) -> &mut dyn Executor;
}

/// Most executors don't care about the control messages, and therefore
/// this method provides a default implementation helper for them.
async fn simple_executor_next<E: SimpleExecutor>(executor: &mut E) -> Result<Message> {
    match executor.input().next().await {
        Ok(message) => match message {
            Message::Chunk(chunk) => executor.consume_chunk(chunk),
            Message::Barrier { epoch, stop } => Ok(Message::Barrier { epoch, stop }),
            Message::Terminate => Ok(Message::Terminate),
        },
        Err(e) => Err(e),
    }
}

/// `StreamConsumer` is the last step in a fragment
#[async_trait]
pub trait StreamConsumer: Send + Sync + 'static {
    /// Run next stream chunk. returns whether the stream is terminated
    async fn next(&mut self) -> Result<bool>;
}
