# Macha account service

This long-running Node service validates the desktop app's Supabase Google
session and creates or looks up the Macha account keyed by Google's stable user
ID. The desktop sends the Supabase access token only from its native Rust
process over HTTPS. The service response contains the Google ID and email, not
session credentials.

## Configure and run

Use Node.js 20.19 or newer. Apply
[`20261103000100_app_users_google_identity.sql`](../supabase/migrations/20261103000100_app_users_google_identity.sql)
to the Supabase project. Configure the host with the variables in
[`.env.example`](.env.example), then run:

```sh
npm start
```

The service listens on `PORT` (8788 by default). Put it behind HTTPS for public
use. `/health` is public; `POST /v1/auth/session` and `GET /v1/auth/me` require
a valid Supabase Google access token. Invalid sessions receive 401. The
service does not log request headers, request bodies, tokens, or upstream error
details.

The Windows app build reads `MACHA_API_URL` from its build environment or the
repository `.env` file. Set it to the HTTPS base URL of this service before
building the app. For local development, `http://127.0.0.1:8788` is accepted.
