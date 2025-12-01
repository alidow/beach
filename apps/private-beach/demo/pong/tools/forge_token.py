import sys
import time
import json
import base64
from cryptography.hazmat.primitives import serialization, hashes
from cryptography.hazmat.primitives.asymmetric import ec

def base64url_encode(data):
    return base64.urlsafe_b64encode(data).rstrip(b'=')

def main():
    key_path = "config/dev-secrets/beach-gate-ec256.pem"
    kid_path = "config/dev-secrets/beach-gate-signing.kid"

    try:
        with open(key_path, "rb") as f:
            private_key = serialization.load_pem_private_key(f.read(), password=None)
        
        with open(kid_path, "r") as f:
            kid = f.read().strip()
    except FileNotFoundError:
        print(f"Error: Keys not found at {key_path} or {kid_path}", file=sys.stderr)
        sys.exit(1)

    header = {
        "alg": "ES256",
        "kid": kid,
        "typ": "JWT"
    }

    now = int(time.time())
    exp = now + 365 * 24 * 3600  # 1 year

    payload = {
        "iss": "beach-gate",
        "sub": "00000000-0000-0000-0000-000000000001",
        "aud": "private-beach-manager",
        "exp": exp,
        "iat": now,
        "entitlements": ["private-beach:turn", "rescue:fallback", "pb:transport.turn"],
        "tier": "standard",
        "profile": "default",
        "email": "mock-user@beach.test",
        "account_id": "00000000-0000-0000-0000-000000000001",
        "scope": "rescue:fallback private-beach:turn pb:beaches.read pb:beaches.write pb:sessions.read pb:sessions.write pb:control.read pb:control.write pb:control.consume pb:agents.onboard pb:harness.publish"
    }

    header_json = json.dumps(header, separators=(',', ':')).encode('utf-8')
    payload_json = json.dumps(payload, separators=(',', ':')).encode('utf-8')

    header_b64 = base64url_encode(header_json)
    payload_b64 = base64url_encode(payload_json)

    signing_input = header_b64 + b'.' + payload_b64
    signature = private_key.sign(signing_input, ec.ECDSA(hashes.SHA256()))
    
    # Convert DER signature to R|S raw format (64 bytes for P-256)
    # cryptography sign() returns DER. We need raw.
    # Actually, let's just use decode_dss_signature to get r and s
    from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature
    r, s = decode_dss_signature(signature)
    
    # Pad r and s to 32 bytes
    r_bytes = r.to_bytes(32, byteorder='big')
    s_bytes = s.to_bytes(32, byteorder='big')
    raw_signature = r_bytes + s_bytes
    
    signature_b64 = base64url_encode(raw_signature)

    token = header_b64 + b'.' + payload_b64 + b'.' + signature_b64
    print(token.decode('utf-8'))

if __name__ == "__main__":
    main()
