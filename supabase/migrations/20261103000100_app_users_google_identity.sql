create table if not exists public.app_users (
  google_id text primary key check (length(google_id) between 1 and 255),
  supabase_user_id uuid not null unique references auth.users(id) on delete cascade,
  email text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

alter table public.app_users enable row level security;
revoke all on public.app_users from public, anon, authenticated;
grant all on public.app_users to service_role;

comment on table public.app_users is 'Macha application users keyed by their stable Google identity ID; service access only.';
