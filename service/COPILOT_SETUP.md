# GitHub Copilot connection setup

1. In GitHub, open **Settings → Developer settings → OAuth Apps → New OAuth App**.
2. Set the application name and homepage URL. The device flow does not use a callback URL.
3. Enable **Device flow** in the OAuth App settings. Copy the Client ID and generate a Client Secret.
4. Apply `supabase/migrations/20261103000200_copilot_accounts.sql` to the Supabase project.
5. Configure the service variables below in the host's secret manager. Do not commit real values.
6. Run the service on a persistent Node host with the matching platform build of the SDK runtime installed.

`offline_access` is requested so GitHub can issue refresh tokens for expiring OAuth tokens. The GitHub account identity endpoint is accessed with the token's default public identity access; no repository or organization scopes are requested.

## Service secrets

- `GITHUB_OAUTH_CLIENT_ID`
- `GITHUB_OAUTH_CLIENT_SECRET` (used only for GitHub's refresh-token exchange)
- `COPILOT_ENCRYPTION_KEY_ID` (ID of the active 32-byte key)
- `COPILOT_ENCRYPTION_KEYS` (JSON object mapping each retained key ID to its base64-encoded 32-byte key)

Generate a key with `openssl rand -base64 32` and inject the result through the host secret manager. Keep encryption keys outside Supabase and the database backups. During rotation, add the new key while retaining old key IDs, set `COPILOT_ENCRYPTION_KEY_ID` to the new ID, and deploy. Records are re-encrypted under the active key when accounts are next checked. Remove the retired key only after every record has rotated.

The environment-backed key provider implements `currentKeyId()` and `keyFor(id)` in `src/copilot-crypto.mjs`; a host KMS provider can replace it while preserving the AES-256-GCM envelope format.
