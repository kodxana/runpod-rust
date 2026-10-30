"""Capture the pinned SDK's upload contracts without sending any HTTP requests."""
import json
import tempfile
from pathlib import Path
from unittest.mock import Mock, mock_open, patch
from runpod.serverless.utils import rp_upload as upload
from s3transfer.utils import ChunksizeAdjuster
import boto3
from boto3.s3.transfer import TransferConfig, S3Transfer
from botocore.config import Config

cases = []
for url in ["", "https://example.com", "https://bucket.s3.us-west-2.amazonaws.com",
            "https://s3.us-west-2.amazonaws.com", "https://bucket.s3.eu-central-003.backblazeb2.com",
            "https://bucket.nyc3.digitaloceanspaces.com", "https://nyc3.digitaloceanspaces.com",
            "https://bucket.NYC3.digitaloceanspaces.com", "https://bucket.nyc3.digitaloceanspaces.com:443",
            "https://bucket.nyc3.digitaloceanspaces.com/path", "//bucket.ams3.digitaloceanspaces.com",
            "bucket.nyc3.digitaloceanspaces.com", "https://BUCKET.NYC3.DIGITALOCEANSPACES.COM",
            "https://example.com/path.s3.odd.name", "https://one.s3.first.s3.second.com",
            "https://example.com/?x=.s3.query.example", ".s3."]:
    cases.append(dict(operation="region", input=url, output=upload.extract_region_from_url(url)))

for size in [0, 1, 25600, 5*1024**2-1, 5*1024**2, 5*1024**2+1, 10000*25600,
             10000*25600+1, 50*1024**3, 5*1024**4]:
    cases.append(dict(operation="chunk", input=size,
                      output=ChunksizeAdjuster().adjust_chunksize(25600, size)))

for bucket in [None, "", "explicit"]:
    for prefix in [None, "", "nested", "ends/"]:
        client = Mock()
        client.generate_presigned_url.return_value = "signed"
        with patch.object(upload, "get_boto_client", return_value=(client, object())), \
                patch.object(upload.time, "strftime", return_value="01-24"), patch.object(upload, "tqdm"):
            result = upload.upload_in_memory_object("a b.txt", b"hello", bucket_name=bucket, prefix=prefix)
        call = client.upload_fileobj.call_args
        cases.append(dict(operation="memory", input=dict(bucket=bucket, prefix=prefix),
                          output=dict(bucket=call.args[1], key=call.args[2],
                                      body=call.args[0].getvalue().decode(), result=result,
                                      expires=client.generate_presigned_url.call_args.kwargs["ExpiresIn"])))

for path in ["image.png", "archive.tar.gz", "UPPER.JPG", "no_extension", ".hidden", "...dots", "name."]:
    client = Mock()
    client.generate_presigned_url.return_value = "signed"
    with patch.object(upload, "get_boto_client", return_value=(client, object())), \
            patch.object(upload.uuid, "uuid4", return_value="11111111-1111-4111-8111-111111111111"), \
            patch("builtins.open", mock_open(read_data=b"image bytes")):
        upload.upload_image("job", path, bucket_name="images")
    call = client.put_object.call_args.kwargs
    cases.append(dict(operation="image", input=path,
                      output=dict(key=call["Key"], content_type=call["ContentType"], body=call["Body"].decode())))

extra = dict(ACL="private", CacheControl="max-age=60", ChecksumAlgorithm="CRC32",
             ContentDisposition="attachment", ContentEncoding="identity", ContentLanguage="en",
             ContentType="text/plain", ExpectedBucketOwner="123456789012", Expires="2030-01-01T00:00:00Z",
             GrantFullControl='id="owner"', GrantRead='id="reader"', GrantReadACP='id="reader-acp"',
             GrantWriteACP='id="writer-acp"', Metadata={"one":"1", "two":"2"},
             ObjectLockLegalHoldStatus="ON", ObjectLockMode="GOVERNANCE", ObjectLockRetainUntilDate="2030-01-01T00:00:00Z",
             RequestPayer="requester", ServerSideEncryption="aws:kms", StorageClass="STANDARD_IA",
             SSECustomerAlgorithm="AES256", SSECustomerKey="my secret key", SSECustomerKeyMD5="provided-md5",
             SSEKMSKeyId="test-key", SSEKMSEncryptionContext="e30=", Tagging="a=b", WebsiteRedirectLocation="/other",
             ChecksumType="FULL_OBJECT", MpuObjectSize=4)
for name in ["CRC32", "CRC32C", "CRC64NVME", "MD5", "SHA1", "SHA256", "SHA512", "XXHASH3", "XXHASH64", "XXHASH128"]:
    extra["Checksum"+name] = "AAAAAA=="
assert set(extra) == set(S3Transfer.ALLOWED_UPLOAD_ARGS)

class StopBeforeNetwork(Exception): pass
headers = {}
client = boto3.client("s3", endpoint_url="http://127.0.0.1:9", aws_access_key_id="TEST_ACCESS",
                      aws_secret_access_key="TEST_SECRET", region_name="us-east-1", config=Config(signature_version="s3v4"))
def capture(request, **_):
    ignored = {"authorization", "x-amz-date", "user-agent", "expect", "x-amz-content-sha256", "amz-sdk-invocation-id", "amz-sdk-request", "content-length"}
    for key, value in request.headers.items():
        if key.lower() not in ignored:
            headers[key.lower()] = value.decode() if isinstance(value, bytes) else str(value)
    raise StopBeforeNetwork()
client.meta.events.register("before-send.s3.PutObject", capture)
with tempfile.TemporaryDirectory() as directory:
    source = Path(directory)/"source"
    source.write_bytes(b"body")
    with patch.object(upload, "get_boto_client", return_value=(client, TransferConfig())), patch.object(upload, "tqdm"):
        try:
            upload.upload_file_to_bucket("name", str(source), bucket_name="bucket", extra_args=extra)
        except StopBeforeNetwork:
            pass
assert headers
cases.append(dict(operation="extra_args", input=extra, output=headers))

path = Path(__file__).resolve().parents[1] / "tests/fixtures/upload.json"
path.write_text(json.dumps(cases, indent=2, ensure_ascii=False)+"\n", encoding="utf-8")
print(f"Captured {len(cases)} Python upload cases")
