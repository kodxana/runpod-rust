"""Capture pinned Python SSRF classifications without making network requests."""
import ipaddress
import json
import os
import random
from pathlib import Path
from unittest.mock import patch

from runpod.serverless.utils import rp_ssrf

cases = []
addresses = {"not-an-ip", "", "127.1", "0177.0.0.1", "1.2.3.004", "0x7f000001",
             "8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:2800:220:1::1",
             "2606:2800:220:1::1%eth0", "fe80::1%eth0", "fec0::1", "::ffff:8.8.8.8"}
for constants in (ipaddress._IPv4Constants, ipaddress._IPv6Constants):
    networks = []
    for value in vars(constants).values():
        if isinstance(value, (ipaddress.IPv4Network, ipaddress.IPv6Network)):
            networks.append(value)
        elif isinstance(value, list):
            networks.extend(value)
    for network in networks:
        for number in {int(network.network_address) - 1, int(network.network_address),
                       int(network.network_address) + 1, int(network.broadcast_address),
                       int(network.broadcast_address) + 1}:
            if 0 <= number < 2 ** network.max_prefixlen:
                address = network.network_address.__class__(number)
                addresses.add(str(address))
                if address.version == 4:
                    addresses.add(f"::ffff:{address}")
rng = random.Random(9167)
for _ in range(96):
    addresses.add(str(ipaddress.IPv4Address(rng.getrandbits(32))))
    addresses.add(str(ipaddress.IPv6Address(rng.getrandbits(128))))
for address in sorted(addresses):
    cases.append(dict(operation="address", input=address, output=rp_ssrf.is_safe_address(address)))

for value in [None, "", "0", "-1", "abc", "1", "12345", " +1_024 ", "_1", "1__2", "1_", "1.0"]:
    environment = {} if value is None else {"RUNPOD_MAX_DOWNLOAD_BYTES": value}
    with patch.dict(os.environ, environment, clear=True):
        cases.append(dict(operation="max_bytes", input=environment, output=rp_ssrf.max_download_bytes()))
for value in ["", "1", "true", "yes", " YES ", "True", "0", "false", "on", "2"]:
    with patch.dict(os.environ, {"RUNPOD_ALLOW_PRIVATE_DOWNLOAD_URLS": value}, clear=True):
        cases.append(dict(operation="protection", input=value, output=rp_ssrf._ssrf_protection_enabled()))
for url in ["http://example.com/path", "http://example.com:8080/path", "http://example.com:80",
            "https://example.com:443/x", "https://93.184.216.34/x", "https://93.184.216.34:8443/x",
            "https://[2606:2800:220:1::1]/x", "https://[2606:2800:220:1::1]:8443/x",
            "https://[2606:2800:220:1::1]:443/x", "http://user:pass@example.com:8080/x"]:
    cases.append(dict(operation="host", input=url, output=rp_ssrf._host_header_for(url)))

path = Path(__file__).resolve().parents[1] / "tests/fixtures/ssrf.json"
path.write_text(json.dumps(cases, indent=2) + "\n", encoding="utf-8")
print(f"Captured {len(cases)} SSRF cases")
