use postcard_schema_ng::Schema;
use postcard_schema_ng::schema::DataModelType;

pub(crate) struct Bytes<const N: usize>;

impl<const N: usize> Schema for Bytes<N> {
    const SCHEMA: &'static DataModelType = &DataModelType::ByteArray { max_len: Some(N) };
}
