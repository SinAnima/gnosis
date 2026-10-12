pub mod filesystem;
pub mod git;
pub mod s3;
pub mod types;
pub mod web;

pub use filesystem::FilesystemConnector;
pub use git::GitContext;
pub use s3::{
    is_s3_uri, parse_s3_uri, AwsS3Backend, MemoryS3Backend, S3Backend, S3Connector, S3Location,
};
pub use types::{GitProtoData, ObjectDescriptor, ProtoData};
