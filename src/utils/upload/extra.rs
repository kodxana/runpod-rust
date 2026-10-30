//! ExtraArgs validation and operation-specific forwarding, matching s3transfer.
use crate::{Error, Result, Value};
use aws_sdk_s3::primitives::{DateTime, DateTimeFormat};
use base64::{Engine, engine::general_purpose::STANDARD};
use md5::{Digest, Md5};
use std::collections::{BTreeMap, HashMap};

pub(super) const CHECKSUMS: &[&str] = &[
    "ChecksumCRC32",
    "ChecksumCRC32C",
    "ChecksumCRC64NVME",
    "ChecksumMD5",
    "ChecksumSHA1",
    "ChecksumSHA256",
    "ChecksumSHA512",
    "ChecksumXXHASH3",
    "ChecksumXXHASH64",
    "ChecksumXXHASH128",
];
const ALLOWED: &[&str] = &[
    "ACL",
    "CacheControl",
    "ContentDisposition",
    "ContentEncoding",
    "ContentLanguage",
    "ContentType",
    "ExpectedBucketOwner",
    "Expires",
    "GrantFullControl",
    "GrantRead",
    "GrantReadACP",
    "GrantWriteACP",
    "Metadata",
    "ObjectLockLegalHoldStatus",
    "ObjectLockMode",
    "ObjectLockRetainUntilDate",
    "RequestPayer",
    "ServerSideEncryption",
    "StorageClass",
    "SSECustomerAlgorithm",
    "SSECustomerKey",
    "SSECustomerKeyMD5",
    "SSEKMSKeyId",
    "SSEKMSEncryptionContext",
    "Tagging",
    "WebsiteRedirectLocation",
    "ChecksumAlgorithm",
    "ChecksumCRC32",
    "ChecksumCRC32C",
    "ChecksumCRC64NVME",
    "ChecksumMD5",
    "ChecksumSHA1",
    "ChecksumSHA256",
    "ChecksumSHA512",
    "ChecksumXXHASH3",
    "ChecksumXXHASH64",
    "ChecksumXXHASH128",
    "ChecksumType",
    "MpuObjectSize",
];

pub(super) fn prepare(
    args: &BTreeMap<String, Value>,
    multipart: bool,
) -> Result<BTreeMap<String, Value>> {
    for key in args.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return Err(Error::InvalidInput(format!(
                "invalid upload ExtraArgs key: {key}"
            )));
        }
    }
    let mut args = args.clone();
    if let Some(key) = args.get("SSECustomerKey").filter(|v| !v.is_null())
        && args.get("SSECustomerKeyMD5").is_none_or(Value::is_null)
    {
        let bytes = string(key, "SSECustomerKey")?.as_bytes();
        let digest = STANDARD.encode(Md5::digest(bytes));
        let encoded = STANDARD.encode(bytes);
        args.insert("SSECustomerKey".into(), encoded.into());
        args.insert("SSECustomerKeyMD5".into(), digest.into());
    }
    if !args.contains_key("ChecksumAlgorithm")
        && !CHECKSUMS.iter().any(|key| args.contains_key(*key))
    {
        args.insert("ChecksumAlgorithm".into(), "CRC32".into());
    }
    if multipart {
        // s3transfer applies these in its fixed checksum order; the last supplied one wins.
        for key in CHECKSUMS {
            if args.contains_key(*key) {
                args.insert("ChecksumType".into(), "FULL_OBJECT".into());
                args.insert(
                    "ChecksumAlgorithm".into(),
                    key.trim_start_matches("Checksum").into(),
                );
            }
        }
    }
    Ok(args)
}
fn invalid(name: &str) -> Error {
    Error::InvalidInput(format!("invalid value for upload ExtraArgs {name}"))
}
fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value.as_str().ok_or_else(|| invalid(name))
}
fn metadata(value: &Value, name: &str) -> Result<HashMap<String, String>> {
    value
        .as_object()
        .ok_or_else(|| invalid(name))?
        .iter()
        .map(|(k, v)| Ok((k.clone(), string(v, name)?.to_owned())))
        .collect()
}
fn date(value: &Value, name: &str) -> Result<DateTime> {
    if let Some(seconds) = value.as_f64() {
        return Ok(DateTime::from_secs_f64(seconds));
    }
    let text = string(value, name)?;
    DateTime::from_str(text, DateTimeFormat::DateTime)
        .or_else(|_| DateTime::from_str(text, DateTimeFormat::HttpDate))
        .map_err(|_| invalid(name))
}

pub(super) fn put(
    mut builder: aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder,
    args: &BTreeMap<String, Value>,
) -> Result<aws_sdk_s3::operation::put_object::builders::PutObjectFluentBuilder> {
    for (name, value) in args {
        builder = match name.as_str() {
            "ACL" => builder.acl(aws_sdk_s3::types::ObjectCannedAcl::from(string(
                value, name,
            )?)),
            "CacheControl" => builder.cache_control(string(value, name)?),
            "ContentDisposition" => builder.content_disposition(string(value, name)?),
            "ContentEncoding" => builder.content_encoding(string(value, name)?),
            "ContentLanguage" => builder.content_language(string(value, name)?),
            "ContentType" => builder.content_type(string(value, name)?),
            "ExpectedBucketOwner" => builder.expected_bucket_owner(string(value, name)?),
            "Expires" => builder.expires(date(value, name)?),
            "GrantFullControl" => builder.grant_full_control(string(value, name)?),
            "GrantRead" => builder.grant_read(string(value, name)?),
            "GrantReadACP" => builder.grant_read_acp(string(value, name)?),
            "GrantWriteACP" => builder.grant_write_acp(string(value, name)?),
            "Metadata" => builder.set_metadata(Some(metadata(value, name)?)),
            "ObjectLockLegalHoldStatus" => builder.object_lock_legal_hold_status(
                aws_sdk_s3::types::ObjectLockLegalHoldStatus::from(string(value, name)?),
            ),
            "ObjectLockMode" => builder.object_lock_mode(aws_sdk_s3::types::ObjectLockMode::from(
                string(value, name)?,
            )),
            "ObjectLockRetainUntilDate" => {
                builder.object_lock_retain_until_date(date(value, name)?)
            }
            "RequestPayer" => {
                builder.request_payer(aws_sdk_s3::types::RequestPayer::from(string(value, name)?))
            }
            "ServerSideEncryption" => builder.server_side_encryption(
                aws_sdk_s3::types::ServerSideEncryption::from(string(value, name)?),
            ),
            "StorageClass" => {
                builder.storage_class(aws_sdk_s3::types::StorageClass::from(string(value, name)?))
            }
            "SSECustomerAlgorithm" => builder.sse_customer_algorithm(string(value, name)?),
            "SSECustomerKey" => builder.sse_customer_key(string(value, name)?),
            "SSECustomerKeyMD5" => builder.sse_customer_key_md5(string(value, name)?),
            "SSEKMSKeyId" => builder.ssekms_key_id(string(value, name)?),
            "SSEKMSEncryptionContext" => builder.ssekms_encryption_context(string(value, name)?),
            "Tagging" => builder.tagging(string(value, name)?),
            "WebsiteRedirectLocation" => builder.website_redirect_location(string(value, name)?),
            "ChecksumAlgorithm" => builder.checksum_algorithm(
                aws_sdk_s3::types::ChecksumAlgorithm::from(string(value, name)?),
            ),
            "ChecksumCRC32" => builder.checksum_crc32(string(value, name)?),
            "ChecksumCRC32C" => builder.checksum_crc32_c(string(value, name)?),
            "ChecksumCRC64NVME" => builder.checksum_crc64_nvme(string(value, name)?),
            "ChecksumMD5" => builder.checksum_md5(string(value, name)?),
            "ChecksumSHA1" => builder.checksum_sha1(string(value, name)?),
            "ChecksumSHA256" => builder.checksum_sha256(string(value, name)?),
            "ChecksumSHA512" => builder.checksum_sha512(string(value, name)?),
            "ChecksumXXHASH3" => builder.checksum_xxhash3(string(value, name)?),
            "ChecksumXXHASH64" => builder.checksum_xxhash64(string(value, name)?),
            "ChecksumXXHASH128" => builder.checksum_xxhash128(string(value, name)?),
            _ => builder,
        };
    }
    Ok(builder)
}

pub(super) fn create(
    mut builder: aws_sdk_s3::operation::create_multipart_upload::builders::CreateMultipartUploadFluentBuilder,
    args: &BTreeMap<String, Value>,
) -> Result<
    aws_sdk_s3::operation::create_multipart_upload::builders::CreateMultipartUploadFluentBuilder,
> {
    for (name, value) in args {
        builder = match name.as_str() {
            "ACL" => builder.acl(aws_sdk_s3::types::ObjectCannedAcl::from(string(
                value, name,
            )?)),
            "CacheControl" => builder.cache_control(string(value, name)?),
            "ContentDisposition" => builder.content_disposition(string(value, name)?),
            "ContentEncoding" => builder.content_encoding(string(value, name)?),
            "ContentLanguage" => builder.content_language(string(value, name)?),
            "ContentType" => builder.content_type(string(value, name)?),
            "ExpectedBucketOwner" => builder.expected_bucket_owner(string(value, name)?),
            "Expires" => builder.expires(date(value, name)?),
            "GrantFullControl" => builder.grant_full_control(string(value, name)?),
            "GrantRead" => builder.grant_read(string(value, name)?),
            "GrantReadACP" => builder.grant_read_acp(string(value, name)?),
            "GrantWriteACP" => builder.grant_write_acp(string(value, name)?),
            "Metadata" => builder.set_metadata(Some(metadata(value, name)?)),
            "ObjectLockLegalHoldStatus" => builder.object_lock_legal_hold_status(
                aws_sdk_s3::types::ObjectLockLegalHoldStatus::from(string(value, name)?),
            ),
            "ObjectLockMode" => builder.object_lock_mode(aws_sdk_s3::types::ObjectLockMode::from(
                string(value, name)?,
            )),
            "ObjectLockRetainUntilDate" => {
                builder.object_lock_retain_until_date(date(value, name)?)
            }
            "RequestPayer" => {
                builder.request_payer(aws_sdk_s3::types::RequestPayer::from(string(value, name)?))
            }
            "ServerSideEncryption" => builder.server_side_encryption(
                aws_sdk_s3::types::ServerSideEncryption::from(string(value, name)?),
            ),
            "StorageClass" => {
                builder.storage_class(aws_sdk_s3::types::StorageClass::from(string(value, name)?))
            }
            "SSECustomerAlgorithm" => builder.sse_customer_algorithm(string(value, name)?),
            "SSECustomerKey" => builder.sse_customer_key(string(value, name)?),
            "SSECustomerKeyMD5" => builder.sse_customer_key_md5(string(value, name)?),
            "SSEKMSKeyId" => builder.ssekms_key_id(string(value, name)?),
            "SSEKMSEncryptionContext" => builder.ssekms_encryption_context(string(value, name)?),
            "Tagging" => builder.tagging(string(value, name)?),
            "WebsiteRedirectLocation" => builder.website_redirect_location(string(value, name)?),
            "ChecksumAlgorithm" => builder.checksum_algorithm(
                aws_sdk_s3::types::ChecksumAlgorithm::from(string(value, name)?),
            ),
            "ChecksumType" => {
                builder.checksum_type(aws_sdk_s3::types::ChecksumType::from(string(value, name)?))
            }
            _ => builder,
        };
    }
    Ok(builder)
}

pub(super) fn part(
    mut builder: aws_sdk_s3::operation::upload_part::builders::UploadPartFluentBuilder,
    args: &BTreeMap<String, Value>,
) -> Result<aws_sdk_s3::operation::upload_part::builders::UploadPartFluentBuilder> {
    for (name, value) in args {
        builder = match name.as_str() {
            "ExpectedBucketOwner" => builder.expected_bucket_owner(string(value, name)?),
            "RequestPayer" => {
                builder.request_payer(aws_sdk_s3::types::RequestPayer::from(string(value, name)?))
            }
            "SSECustomerAlgorithm" => builder.sse_customer_algorithm(string(value, name)?),
            "SSECustomerKey" => builder.sse_customer_key(string(value, name)?),
            "SSECustomerKeyMD5" => builder.sse_customer_key_md5(string(value, name)?),
            "ChecksumAlgorithm" => builder.checksum_algorithm(
                aws_sdk_s3::types::ChecksumAlgorithm::from(string(value, name)?),
            ),
            _ => builder,
        };
    }
    Ok(builder)
}

pub(super) fn complete(mut builder: aws_sdk_s3::operation::complete_multipart_upload::builders::CompleteMultipartUploadFluentBuilder, args: &BTreeMap<String, Value>) -> Result<aws_sdk_s3::operation::complete_multipart_upload::builders::CompleteMultipartUploadFluentBuilder>{
    for (name, value) in args {
        builder =
            match name.as_str() {
                "ExpectedBucketOwner" => builder.expected_bucket_owner(string(value, name)?),
                "RequestPayer" => builder
                    .request_payer(aws_sdk_s3::types::RequestPayer::from(string(value, name)?)),
                "SSECustomerAlgorithm" => builder.sse_customer_algorithm(string(value, name)?),
                "SSECustomerKey" => builder.sse_customer_key(string(value, name)?),
                "SSECustomerKeyMD5" => builder.sse_customer_key_md5(string(value, name)?),
                "ChecksumCRC32" => builder.checksum_crc32(string(value, name)?),
                "ChecksumCRC32C" => builder.checksum_crc32_c(string(value, name)?),
                "ChecksumCRC64NVME" => builder.checksum_crc64_nvme(string(value, name)?),
                "ChecksumMD5" => builder.checksum_md5(string(value, name)?),
                "ChecksumSHA1" => builder.checksum_sha1(string(value, name)?),
                "ChecksumSHA256" => builder.checksum_sha256(string(value, name)?),
                "ChecksumSHA512" => builder.checksum_sha512(string(value, name)?),
                "ChecksumXXHASH3" => builder.checksum_xxhash3(string(value, name)?),
                "ChecksumXXHASH64" => builder.checksum_xxhash64(string(value, name)?),
                "ChecksumXXHASH128" => builder.checksum_xxhash128(string(value, name)?),
                "ChecksumType" => builder
                    .checksum_type(aws_sdk_s3::types::ChecksumType::from(string(value, name)?)),
                "MpuObjectSize" => {
                    builder.mpu_object_size(value.as_i64().ok_or_else(|| invalid(name))?)
                }
                _ => builder,
            };
    }
    Ok(builder)
}
