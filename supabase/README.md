# Supabase migration setup

For account-based Remote mode, run both files, in filename order, from
[`20261025000100_remote_account_devices.sql`](migrations/20261025000100_remote_account_devices.sql)
and [`20261025000200_remote_realtime_broadcast_policy.sql`](migrations/20261025000200_remote_realtime_broadcast_policy.sql)
in the Supabase SQL Editor or Supabase CLI. The first creates `public.remote_devices` and
`public.remote_queued_messages` so it can coexist with the older
`public.devices` and `public.queued_messages` tables from `setup.sql`.

`setup.sql` is retained only as legacy SQL for existing locally paired installs.
It is not part of the Google account Remote flow and is not exposed in the app.

## Troubleshooting `PGRST202` / `register_device`

If the app reports that `public.register_device` was not found, Google sign-in
has completed but the Remote database RPC is not visible to the Data API. Run
both migration files above in order in the developer-owned project. The first
migration defines `register_device(p_type, p_name, p_pubkey)` and now notifies
PostgREST to reload its schema cache. If the migrations were already applied,
run this once in the Supabase SQL Editor:

```sql
notify pgrst, 'reload schema';
```

This is a backend setup step; rebuilding or reinstalling the Windows app does
not create the missing database function.

## Recover devices marked revoked or an expired Google session

Run `20261027000100_remote_device_reenrollment_recovery.sql` in the developer-owned
Supabase project. Signing out now preserves device trust; explicit **Revoke this
device** controls remove it. When an installation signs in again, a revoked or
pending device is auto-approved only if no other approved device exists. If a
trusted device remains, it must approve the re-enrolling device in Settings.

On Android, sign in with Google again from Settings if the saved session is
reported expired. On Windows, use **Sign in again to reconnect this laptop**
when the current laptop status is revoked. After both installations show
`approved`, choose **Connect using Macha Remote** on Android to start the account
link. Nearby QR pairing remains independent.
