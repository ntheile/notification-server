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
APNs payload containing:

- `protocol`
- `version`
- `relay`
- `event_id`
- `wallet_service_pubkey`

When the full event fits inside APNs limits, the payload may also include
`nwc_event`; otherwise the phone can refetch by `event_id`.

It does not need the NWC secret, wallet private key, decrypted request, invoice,
amount, memo, or balance.

Configure APNs token auth with:

```env
APNS_TEAM_ID=...
APNS_KEY_ID=...
APNS_PRIVATE_KEY_PATH=/absolute/path/AuthKey_XXXXXXXXXX.p8
# or APNS_PRIVATE_KEY="-----BEGIN PRIVATE KEY-----..."
```

Register a wallet app install/NWC connection:

```http
POST /register-nwc-push
Content-Type: application/json
Authorization: Nostr <base64-kind-27235-event>

{
  "id": "install-id",
  "push_service": "apns",
  "push_token": "apns-token",
  "app_id": "com.wallet.example",
  "environment": "sandbox",
  "client_pubkey": "nwc-client-pubkey",
  "wallet_service_pubkey": "wallet-service-pubkey",
  "relay": "wss://relay.getalby.com/v1",
  "name": "Alby Go",
  "enabled": true
}
```

The `Authorization` event follows NIP-98-style HTTP auth. It must be signed by
`wallet_service_pubkey`, be kind `27235`, and include `u`, `method`, and
`payload` tags for the request URL, `POST`, and the SHA-256 hash of the JSON
body.

NWC push registrations are stored in the generic `nwc_push_registrations` table.
iOS/APNS uses `push_service = "apns"` with the APNS device token in
`push_token`. Android/FCM is reserved in the schema, but the API currently
rejects `push_service = "fcm"` until FCM delivery is wired up.

The spec wake endpoint is also available at:

```http
POST /.well-known/nostr/nwc-wake
```

The wallet app will call `/register-nwc-push` automatically when
`NWC_WAKE_SERVER_URL` is set in the iOS build environment.
