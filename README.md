# mutiny-notifications

Push notification server for mutiny.

To generate keys, run:

```bash
npm install web-push -g
web-push generate-vapid-keys
```

then create a `.env` file from the `.env.sample` and set `VAPID_KEY` equal to your generated private key.

Your public key will be need to be used when building mutiny-web.

## NWC Wake for Rebel Wallet

The server can also act as a privacy-preserving NWC wake provider for Rebel
Wallet. It listens for registered `kind:23194` NWC request events and sends an
APNs payload containing only:

- `protocol`
- `version`
- `relay`
- `event_id`
- `wallet_service_pubkey`

It does not need the NWC secret, wallet private key, decrypted request, invoice,
amount, memo, or balance.

Configure APNs token auth with:

```env
APNS_TEAM_ID=...
APNS_KEY_ID=...
APNS_PRIVATE_KEY_PATH=/absolute/path/AuthKey_XXXXXXXXXX.p8
# or APNS_PRIVATE_KEY="-----BEGIN PRIVATE KEY-----..."
```

Register a Rebel install/NWC connection:

```http
POST /register-apns-nwc
Content-Type: application/json

{
  "id": "install-id",
  "device_token": "apns-token",
  "bundle_id": "com.nicktee.rebelwallet",
  "environment": "sandbox",
  "author": "nwc-client-pubkey",
  "tagged": "wallet-service-pubkey",
  "relay": "wss://relay.getalby.com/v1",
  "name": "Alby Go",
  "enabled": true
}
```

The spec wake endpoint is also available at:

```http
POST /.well-known/nostr/nwc-wake
```

Rebel Wallet will call `/register-apns-nwc` automatically when
`NWC_WAKE_SERVER_URL` is set in the iOS build environment.
