# mutiny-notifications

Push notification server for mutiny.

To generate keys, run:

```bash
npm install web-push -g
web-push generate-vapid-keys
```

then create a `.env` file from the `.env.sample` and set `VAPID_KEY` equal to your generated private key.

Your public key will be need to be used when building mutiny-web.
