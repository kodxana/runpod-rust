"""Generate PUBLIC TEST-ONLY TLS material; never use these keys in a deployment.

Optional regeneration requires cryptography. SDK tests consume the checked-in DER
files and do not run Python or openssl. Certificates cover 2020 through 2045.
"""
from datetime import datetime, timezone
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

path = Path(__file__).resolve().parents[1] / "tests/fixtures/tls"
path.mkdir(exist_ok=True)
root_key = ec.generate_private_key(ec.SECP256R1())
leaf_key = ec.generate_private_key(ec.SECP256R1())
root_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Runpod Rust PUBLIC TEST CA")])
leaf_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "download.test")])

def certificate(subject, public_key):
    return (x509.CertificateBuilder().subject_name(subject).issuer_name(root_name)
            .public_key(public_key).serial_number(x509.random_serial_number())
            .not_valid_before(datetime(2020, 1, 1, tzinfo=timezone.utc))
            .not_valid_after(datetime(2045, 1, 1, tzinfo=timezone.utc)))

root = (certificate(root_name, root_key.public_key())
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
        .add_extension(x509.KeyUsage(True, False, False, False, False, True, True, False, False), critical=True)
        .sign(root_key, hashes.SHA256()))
leaf = (certificate(leaf_name, leaf_key.public_key())
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(x509.SubjectAlternativeName([x509.DNSName("download.test")]), critical=False)
        .add_extension(x509.ExtendedKeyUsage([x509.oid.ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
        .sign(root_key, hashes.SHA256()))
for name, cert in [("root", root), ("server", leaf)]:
    (path / f"{name}.der").write_bytes(cert.public_bytes(serialization.Encoding.DER))
(path / "PUBLIC_TEST_ONLY_key.der").write_bytes(leaf_key.private_bytes(
    serialization.Encoding.DER, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
print("Wrote public test TLS fixtures")
