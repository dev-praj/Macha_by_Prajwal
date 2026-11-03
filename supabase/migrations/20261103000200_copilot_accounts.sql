create table if not exists public.copilot_accounts (
  account_id uuid primary key,
  supabase_user_id uuid not null references auth.users(id) on delete cascade,
  github_user_id bigint not null,
  github_login text not null,
  status text not null check (status in ('connected', 'needs_reconnect')),
  subscription_status text not null default 'unverified'
    check (subscription_status in ('unverified', 'active', 'inactive')),
  encryption_key_id text not null,
  encrypted_token jsonb not null,
  wrapped_dek jsonb not null,
  token_expires_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  unique (supabase_user_id, github_user_id)
);

alter table public.copilot_accounts enable row level security;
revoke all on public.copilot_accounts from public, anon, authenticated;
grant all on public.copilot_accounts to service_role;

comment on table public.copilot_accounts is
  'Copilot OAuth credentials stored only as AES-256-GCM encrypted envelopes; service-role access only.';
