use super::{S3Client, TransferConfig, UploadError, extra};
use crate::{Error, Result, Value};
use aws_sdk_s3::{
    primitives::{ByteStream, Length},
    types::{CompletedMultipartUpload, CompletedPart},
};
use futures_util::{StreamExt, TryStreamExt, stream};
use std::{collections::BTreeMap, path::Path};

/// s3transfer's adjustment: fit at most 10,000 parts, then clamp to 5 MiB..=5 GiB.
pub fn multipart_chunk_size(size: u64, configured: u64) -> Result<u64> {
    if configured == 0 {
        return Err(Error::InvalidInput(
            "multipart chunk size must be positive".into(),
        ));
    }
    let mut chunk = configured;
    while size.div_ceil(chunk) > 10_000 {
        chunk = chunk.saturating_mul(2);
    }
    Ok(chunk.clamp(5 * 1024 * 1024, 5 * 1024 * 1024 * 1024))
}

#[derive(Clone, Copy)]
pub(super) enum Source<'a> {
    File(&'a Path),
    Memory(&'a [u8]),
}
impl Source<'_> {
    async fn body(self, offset: u64, size: u64) -> Result<ByteStream> {
        match self {
            Self::File(path) => ByteStream::read_from()
                .path(path)
                .offset(offset)
                .length(Length::Exact(size))
                .build()
                .await
                .map_err(|e| UploadError::local("read upload", e)),
            Self::Memory(data) => Ok(ByteStream::from(
                data[offset as usize..(offset + size) as usize].to_vec(),
            )),
        }
    }
}

pub(super) async fn upload(
    s3: &S3Client,
    bucket: &str,
    key: &str,
    source: Source<'_>,
    size: u64,
    args: &BTreeMap<String, Value>,
    config: &TransferConfig,
) -> Result<()> {
    let multipart = size >= config.multipart_threshold;
    let args = extra::prepare(args, multipart)?;
    if !multipart {
        extra::put(
            s3.put_object()
                .bucket(bucket)
                .key(key)
                .body(source.body(0, size).await?),
            &args,
        )?
        .send()
        .await
        .map_err(|e| UploadError::sdk("put object", e))?;
        return Ok(());
    }
    let chunk = multipart_chunk_size(size, config.multipart_chunksize)?;
    let count = size.div_ceil(chunk);
    if count > 10_000 {
        return Err(Error::InvalidInput(
            "upload exceeds 10,000 maximum-sized S3 parts".into(),
        ));
    }
    let created = extra::create(s3.create_multipart_upload().bucket(bucket).key(key), &args)?
        .send()
        .await
        .map_err(|e| UploadError::sdk("create multipart upload", e))?;
    let id = created
        .upload_id
        .ok_or_else(|| Error::InvalidResponse("multipart response has no UploadId".into()))?;
    let mut cleanup = AbortUpload(Some(Abort {
        s3: s3.clone(),
        bucket: bucket.into(),
        key: key.into(),
        id: id.clone(),
        expected_owner: args
            .get("ExpectedBucketOwner")
            .and_then(Value::as_str)
            .map(str::to_owned),
        request_payer: args
            .get("RequestPayer")
            .and_then(Value::as_str)
            .map(aws_sdk_s3::types::RequestPayer::from),
    }));
    let result = async {
        let mut parts: Vec<CompletedPart> = stream::iter(0..count)
            .map(|index| {
                let id = &id;
                let args = &args;
                async move {
                    let offset = index * chunk;
                    let body = source.body(offset, chunk.min(size - offset)).await?;
                    let part_number = (index + 1) as i32;
                    let output = extra::part(
                        s3.upload_part()
                            .bucket(bucket)
                            .key(key)
                            .upload_id(id)
                            .part_number(part_number)
                            .body(body),
                        args,
                    )?
                    .send()
                    .await
                    .map_err(|e| UploadError::sdk("upload part", e))?;
                    if output.e_tag.is_none() {
                        return Err(Error::InvalidResponse("uploaded part has no ETag".into()));
                    }
                    Ok(CompletedPart::builder()
                        .part_number(part_number)
                        .set_e_tag(output.e_tag)
                        .set_checksum_crc32(output.checksum_crc32)
                        .set_checksum_crc32_c(output.checksum_crc32_c)
                        .set_checksum_crc64_nvme(output.checksum_crc64_nvme)
                        .set_checksum_md5(output.checksum_md5)
                        .set_checksum_sha1(output.checksum_sha1)
                        .set_checksum_sha256(output.checksum_sha256)
                        .set_checksum_sha512(output.checksum_sha512)
                        .set_checksum_xxhash3(output.checksum_xxhash3)
                        .set_checksum_xxhash64(output.checksum_xxhash64)
                        .set_checksum_xxhash128(output.checksum_xxhash128)
                        .build())
                }
            })
            .buffer_unordered(config.max_concurrency.get())
            .try_collect()
            .await?;
        parts.sort_by_key(|part| part.part_number);
        extra::complete(
            s3.complete_multipart_upload()
                .bucket(bucket)
                .key(key)
                .upload_id(&id)
                .multipart_upload(
                    CompletedMultipartUpload::builder()
                        .set_parts(Some(parts))
                        .build(),
                ),
            &args,
        )?
        .send()
        .await
        .map_err(|e| UploadError::sdk("complete multipart upload", e))?;
        Ok(())
    }
    .await;
    if result.is_ok() {
        cleanup.0 = None;
    } else if let Some(abort) = cleanup.0.take() {
        abort.run().await;
    }
    result
}

struct Abort {
    s3: S3Client,
    bucket: String,
    key: String,
    id: String,
    expected_owner: Option<String>,
    request_payer: Option<aws_sdk_s3::types::RequestPayer>,
}
impl Abort {
    async fn run(self) {
        if self
            .s3
            .abort_multipart_upload()
            .bucket(self.bucket)
            .key(self.key)
            .upload_id(self.id)
            .set_expected_bucket_owner(self.expected_owner)
            .set_request_payer(self.request_payer)
            .send()
            .await
            .is_err()
        {
            tracing::warn!("S3 multipart abort failed");
        }
    }
}
struct AbortUpload(Option<Abort>);
impl Drop for AbortUpload {
    fn drop(&mut self) {
        if let Some(abort) = self.0.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(abort.run());
        }
    }
}
