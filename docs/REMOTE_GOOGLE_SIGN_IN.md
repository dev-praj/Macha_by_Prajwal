# Remote Google sign-in (developer setup)

Macha Remote uses one Supabase project owned by the Macha developer. Users only
sign in with Google; their apps do not ask for a Supabase URL, key, or SQL.
Nearby QR pairing remains local and does not require sign-in.

## Configure the shared project

1. In Google Cloud, create a project, open **Google Auth Platform → Branding**
   to configure the consent screen, then open **Google Auth Platform → Clients**
   and create one OAuth client of type **Web application**.
2. In the Supabase project, open the Google provider page under
   **Authentication → Sign In / Providers → Google**. Enable Google and enter
   the OAuth Web client ID and client secret.
   Add the Supabase callback shown on that page (usually
   `https://<project-ref>.supabase.co/auth/v1/callback`) to the Google OAuth
   client's **Authorized redirect URIs**. This must be the exact Supabase
   callback URL; do not enter `http://127.0.0.1:*/oauth/callback` or the Android
   `macha://` URL in Google Cloud. Keep the Google client secret only in Supabase
   Dashboard.
3. In Supabase **Authentication → URL Configuration → Redirect URLs**, allow
   Android `macha://remote-auth/callback*` and Windows
   `http://127.0.0.1:*/oauth/callback`.
   These are app destination URLs and belong in Supabase, not in Google's
   Authorized redirect URIs. Supabase supports wildcard patterns in this list.
   If Android lands on a page such as
   `http://localhost:3000`, Supabase fell back to its Site URL because the app's
   `redirect_to` did not match an allowed URL; add the Android entry exactly and
   retry from a fresh sign-in.
4. Run the migration files under [`supabase/migrations/`](../supabase/migrations/)
   in filename order in the developer-owned project. The second migration
   tightens private Broadcast policy checks; do not edit or enable RLS on
   Supabase's managed `realtime.messages` table. In Supabase Realtime settings,
   turn off public channel access. The app uses the system browser and OAuth
   authorization-code PKCE. Never place a service-role/secret key or database
   password in a client or repository.

## Build configuration

The two client build values are public Supabase client configuration. They are
not user secrets; authorization is enforced by Supabase Auth, RLS, and the
approved-device protocol.

- Windows: copy `.env.example` to `.env` in the repository root and set
  `SUPABASE_URL` and `SUPABASE_ANON_KEY`. `windows/scripts/tauri.mjs` passes
  these values to the native build without echoing them.
- Android: add the same two properties from `android/local.properties.example`
  to `android/local.properties` (keep any existing `sdk.dir` entry). Gradle
  embeds them into `BuildConfig`; `local.properties` is ignored by Git. The
  `Macha-android` mirror reads its own `local.properties` when built from that
  directory.

If either value is missing, the build still supports nearby pairing but
Remote sign-in reports that Remote is unavailable in that build. Do not add a
Google client ID/secret, Supabase service-role/secret key, or database password
to either app. Before public release, publish a privacy policy and complete
Google OAuth verification; in testing mode, only listed test users can sign in.
