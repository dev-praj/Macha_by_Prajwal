-- Macha Supabase cloud relay schema (version 3).
-- Safe to run more than once in the Supabase SQL Editor.
create extension if not exists pgcrypto;
-- Supabase normally installs pgcrypto in `extensions`. Normalize older or
-- manually enabled projects so SECURITY DEFINER functions can qualify it.
create schema if not exists extensions;
do $$
declare
    installed_schema text;
begin
    select namespace.nspname into installed_schema
    from pg_catalog.pg_extension extension_row
    join pg_catalog.pg_namespace namespace on namespace.oid = extension_row.extnamespace
    where extension_row.extname = 'pgcrypto';
    if installed_schema <> 'extensions' then
        alter extension pgcrypto set schema extensions;
    end if;
end;
$$;

create table if not exists public.links (
    id uuid primary key default gen_random_uuid(),
    created_at timestamptz not null default now(),
    revoked boolean not null default false
);

alter table public.links add column if not exists revoked boolean not null default false;

create table if not exists public.pairing_sessions (
    id uuid primary key default gen_random_uuid(),
    token_hash bytea not null,
    laptop_pubkey text not null,
    laptop_user_id uuid not null,
    phone_user_id uuid,
    phone_pubkey text,
    link_id uuid references public.links(id) on delete cascade,
    expires_at timestamptz not null,
    used boolean not null default false
);

create table if not exists public.devices (
    id uuid primary key default gen_random_uuid(),
    link_id uuid not null references public.links(id) on delete cascade,
    user_id uuid not null,
    type text not null check (type in ('laptop', 'phone')),
    pubkey text not null,
    push_token text,
    last_seen timestamptz not null default now(),
    revoked boolean not null default false,
    unique (link_id, type)
);

create table if not exists public.queued_messages (
    id uuid primary key,
    link_id uuid not null references public.links(id) on delete cascade,
    from_device uuid not null references public.devices(id) on delete cascade,
    to_device uuid not null references public.devices(id) on delete cascade,
    message_type text not null default 'data',
    seq bigint not null default 0,
    sent_at bigint not null default 0,
    ciphertext text not null,
    nonce text not null,
    created_at timestamptz not null default now(),
    expires_at timestamptz not null
);

alter table public.queued_messages add column if not exists from_device uuid references public.devices(id) on delete cascade;
alter table public.queued_messages add column if not exists message_type text not null default 'data';
alter table public.queued_messages add column if not exists seq bigint not null default 0;
alter table public.queued_messages add column if not exists sent_at bigint not null default 0;

-- Failed token attempts are tracked per Google account so guessing cannot be
-- repeated indefinitely. The client has no direct access to this table.
create table if not exists public.pair_claim_attempts (
    user_id uuid primary key,
    window_started_at timestamptz not null default now(),
    attempts integer not null default 0
);

create table if not exists public.realtime_probes (
    id uuid primary key default gen_random_uuid(),
    link_id uuid not null unique references public.links(id) on delete cascade,
    user_id uuid not null,
    expires_at timestamptz not null default (now() + interval '2 minutes')
);

create index if not exists queued_messages_recipient_expiry_idx
    on public.queued_messages (to_device, expires_at);
create index if not exists pairing_sessions_expiry_idx
    on public.pairing_sessions (expires_at);

alter table public.pairing_sessions enable row level security;
alter table public.links enable row level security;
alter table public.devices enable row level security;
alter table public.queued_messages enable row level security;
alter table public.pair_claim_attempts enable row level security;
alter table public.realtime_probes enable row level security;

revoke all on public.pairing_sessions, public.links, public.devices, public.queued_messages, public.pair_claim_attempts, public.realtime_probes from public, anon, authenticated;
grant select on public.devices, public.links to authenticated;
grant select, insert, delete on public.queued_messages to authenticated;

create or replace function public.qb_is_link_member(target_link uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
    select auth.uid() is not null and exists (
        select 1 from public.devices as mine
        join public.links as active_link on active_link.id = mine.link_id
        where mine.link_id = target_link
          and mine.user_id = (select auth.uid())
          and mine.revoked = false
          and active_link.revoked = false
    )
$$;

revoke all on function public.qb_is_link_member(uuid) from public, anon;
grant execute on function public.qb_is_link_member(uuid) to authenticated;

drop policy if exists links_read_link_members on public.links;
create policy links_read_link_members on public.links
    for select to authenticated
    using (public.qb_is_link_member(id) and revoked = false);

drop policy if exists devices_read_link_members on public.devices;
create policy devices_read_link_members on public.devices
    for select to authenticated
    using (public.qb_is_link_member(link_id) and devices.revoked = false);

drop policy if exists queued_messages_insert_own_link on public.queued_messages;
create policy queued_messages_insert_own_link on public.queued_messages
    for insert to authenticated
    with check (
        exists (
            select 1
            from public.devices as sender
            join public.devices as recipient
              on recipient.id = queued_messages.to_device
             and recipient.link_id = sender.link_id
            join public.links as paired_link
              on paired_link.id = sender.link_id
            where sender.id = queued_messages.from_device
              and sender.link_id = queued_messages.link_id
              and sender.user_id = (select auth.uid())
              and sender.revoked = false
              and recipient.revoked = false
              and paired_link.revoked = false
        )
    );

drop policy if exists queued_messages_read_recipient on public.queued_messages;
create policy queued_messages_read_recipient on public.queued_messages
    for select to authenticated
    using (
        exists (
            select 1 from public.devices as recipient
            join public.links as paired_link on paired_link.id = recipient.link_id
            where recipient.id = queued_messages.to_device
              and recipient.user_id = (select auth.uid())
              and recipient.link_id = queued_messages.link_id
              and recipient.revoked = false
              and paired_link.revoked = false
        )
    );

drop policy if exists queued_messages_delete_recipient on public.queued_messages;
create policy queued_messages_delete_recipient on public.queued_messages
    for delete to authenticated
    using (
        exists (
            select 1 from public.devices as recipient
            where recipient.id = queued_messages.to_device
              and recipient.user_id = (select auth.uid())
              and recipient.link_id = queued_messages.link_id
              and recipient.revoked = false
        )
    );

create or replace function public.qb_schema_version()
returns integer
language plpgsql
security definer
set search_path = ''
as $$
begin
    if auth.uid() is null then raise exception 'authentication_required'; end if;
    return 3;
end;
$$;

-- OAuth provider metadata is assigned by Supabase Auth, not trusted from the client.
-- Requiring a verified Google identity makes auth.uid() the shared-account boundary.
create or replace function public.qb_is_google_user(target_user uuid)
returns boolean
language sql
stable
security definer
set search_path = ''
as $$
    select exists (
        select 1 from auth.users as account
        where account.id = target_user
          and account.email_confirmed_at is not null
          and account.raw_app_meta_data ->> 'provider' = 'google'
    )
$$;

revoke all on function public.qb_is_google_user(uuid) from public, anon;

create or replace function public.cleanup()
returns integer
language plpgsql
security definer
set search_path = ''
as $$
declare
    removed integer;
begin
    if auth.uid() is null then raise exception 'authentication_required'; end if;
    if not public.qb_is_google_user(auth.uid()) then raise exception 'google_account_required'; end if;
    delete from public.queued_messages where expires_at <= now();
    get diagnostics removed = row_count;
    delete from public.pairing_sessions where expires_at <= now();
    delete from public.pair_claim_attempts where window_started_at < now() - interval '1 hour';
    delete from public.realtime_probes where expires_at <= now();
    return removed;
end;
$$;

create or replace function public.pair_start(laptop_pubkey text)
returns jsonb
language plpgsql
security definer
set search_path = ''
as $$
declare
    raw_token bytea;
    pairing_id uuid;
    token_text text;
begin
    if auth.uid() is null then raise exception 'authentication_required'; end if;
    if not public.qb_is_google_user(auth.uid()) then raise exception 'google_account_required'; end if;
    if laptop_pubkey !~ '^[A-Za-z0-9_-]{43}$' then raise exception 'invalid_public_key'; end if;
    perform public.cleanup();
    raw_token := extensions.gen_random_bytes(32);
    token_text := rtrim(translate(encode(raw_token, 'base64'), '+/', '-_'), '=');
    insert into public.pairing_sessions (token_hash, laptop_pubkey, laptop_user_id, expires_at)
    values (extensions.digest(convert_to(token_text, 'UTF8'), 'sha256'), laptop_pubkey, auth.uid(), now() + interval '10 minutes')
    returning id into pairing_id;
    return jsonb_build_object('pairing_id', pairing_id, 'token', token_text);
end;
$$;

create or replace function public.pair_claim(pairing_id uuid, token text, phone_pubkey text)
returns jsonb
language plpgsql
security definer
set search_path = ''
as $$
declare
    session_row public.pairing_sessions%rowtype;
    new_link uuid;
    laptop_device uuid;
    phone_device uuid;
begin
    if auth.uid() is null then raise exception 'authentication_required'; end if;
    if not public.qb_is_google_user(auth.uid()) then raise exception 'google_account_required'; end if;
    if phone_pubkey !~ '^[A-Za-z0-9_-]{43}$' then raise exception 'invalid_public_key'; end if;

    -- Persist failed claims by returning an error object instead of raising (an
    -- exception would roll this transaction and its counter update back).
    insert into public.pair_claim_attempts(user_id, window_started_at, attempts)
    values (auth.uid(), now(), 0)
    on conflict (user_id) do update
       set window_started_at = case
               when public.pair_claim_attempts.window_started_at < now() - interval '10 minutes' then now()
               else public.pair_claim_attempts.window_started_at
           end,
           attempts = case
               when public.pair_claim_attempts.window_started_at < now() - interval '10 minutes' then 0
               else public.pair_claim_attempts.attempts
           end;
    if (select attempts from public.pair_claim_attempts where user_id = auth.uid()) >= 8 then
        return jsonb_build_object('error', 'invalid_or_expired');
    end if;

    -- The QR creator and phone must authenticate as the exact same Supabase Google user.
    if not exists (
        select 1 from public.pairing_sessions as pending
        where pending.id = pair_claim.pairing_id
          and pending.laptop_user_id = auth.uid()
          and pending.used = false
          and pending.expires_at > now()
    ) then
        return jsonb_build_object('error', 'same_google_account_required');
    end if;

    -- The conditional UPDATE is the single-use claim gate: only one device can win.
    update public.pairing_sessions
       set used = true, phone_user_id = auth.uid(), phone_pubkey = pair_claim.phone_pubkey
     where id = pair_claim.pairing_id
       and used = false
       and expires_at > now()
       and token_hash = extensions.digest(convert_to(pair_claim.token, 'UTF8'), 'sha256')
    returning * into session_row;
    if not found then
        update public.pair_claim_attempts set attempts = attempts + 1 where user_id = auth.uid();
        return jsonb_build_object('error', 'invalid_or_expired');
    end if;

    insert into public.links default values returning id into new_link;
    insert into public.devices (link_id, user_id, type, pubkey)
    values (new_link, session_row.laptop_user_id, 'laptop', session_row.laptop_pubkey)
    returning id into laptop_device;
    insert into public.devices (link_id, user_id, type, pubkey)
    values (new_link, auth.uid(), 'phone', session_row.phone_pubkey)
    returning id into phone_device;
    update public.pairing_sessions set link_id = new_link where id = session_row.id;

    return jsonb_build_object(
        'link_id', new_link,
        'laptop_pubkey', session_row.laptop_pubkey,
        'device_id', phone_device,
        'laptop_device_id', laptop_device
    );
end;
$$;

create or replace function public.pair_status(pairing_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = ''
as $$
declare
    session_row public.pairing_sessions%rowtype;
    laptop_device uuid;
    phone_device uuid;
begin
    if auth.uid() is null then raise exception 'authentication_required'; end if;
    select * into session_row from public.pairing_sessions
    where id = pair_status.pairing_id and laptop_user_id = auth.uid();
    if not found then raise exception 'pairing_not_found'; end if;
    if session_row.link_id is not null then
        select id into laptop_device from public.devices where link_id = session_row.link_id and type = 'laptop';
        select id into phone_device from public.devices where link_id = session_row.link_id and type = 'phone';
    end if;
    return jsonb_build_object(
        'claimed', session_row.used and session_row.link_id is not null,
        'link_id', session_row.link_id,
        'phone_pubkey', session_row.phone_pubkey,
        'device_id', laptop_device,
        'phone_device_id', phone_device
    );
end;
$$;

create or replace function public.unpair()
returns integer
language plpgsql
security definer
set search_path = ''
as $$
declare
    removed integer;
begin
    if auth.uid() is null then raise exception 'authentication_required'; end if;
    update public.devices set revoked = true
     where user_id = auth.uid() and revoked = false;
    update public.links set revoked = true
     where id in (select link_id from public.devices where user_id = auth.uid());
    delete from public.queued_messages
     where link_id in (select link_id from public.devices where user_id = auth.uid());
    get diagnostics removed = row_count;
    delete from public.links
     where id in (select link_id from public.devices where user_id = auth.uid());
    return removed;
end;
$$;

revoke all on function public.qb_schema_version() from public, anon;
revoke all on function public.cleanup() from public, anon;
revoke all on function public.pair_start(text) from public, anon;
revoke all on function public.pair_claim(uuid, text, text) from public, anon;
revoke all on function public.pair_status(uuid) from public, anon;
revoke all on function public.unpair() from public, anon;
grant execute on function public.qb_schema_version() to authenticated;
grant execute on function public.cleanup() to authenticated;
grant execute on function public.pair_start(text) to authenticated;
grant execute on function public.pair_claim(uuid, text, text) to authenticated;
grant execute on function public.pair_status(uuid) to authenticated;
grant execute on function public.unpair() to authenticated;

-- A short-lived link lets the first-run wizard exercise private Broadcast
-- authorization before a real phone is paired. It carries no user data.
create or replace function public.qb_realtime_test_start()
returns uuid
language plpgsql
security definer
set search_path = ''
as $$
declare
    new_link uuid;
begin
    if auth.uid() is null then raise exception 'authentication_required'; end if;
    insert into public.links default values returning id into new_link;
    insert into public.devices (link_id, user_id, type, pubkey)
    values (new_link, auth.uid(), 'laptop', 'probe');
    insert into public.realtime_probes(link_id, user_id)
    values (new_link, auth.uid());
    return new_link;
end;
$$;

create or replace function public.qb_realtime_test_finish(probe_link uuid)
returns boolean
language plpgsql
security definer
set search_path = ''
as $$
begin
    if auth.uid() is null then raise exception 'authentication_required'; end if;
    delete from public.realtime_probes
    where link_id = probe_link and user_id = auth.uid();
    if not found then return false; end if;
    delete from public.links where id = probe_link;
    return true;
end;
$$;

revoke all on function public.qb_realtime_test_start() from public, anon;
revoke all on function public.qb_realtime_test_finish(uuid) from public, anon;
grant execute on function public.qb_realtime_test_start() to authenticated;
grant execute on function public.qb_realtime_test_finish(uuid) to authenticated;

drop policy if exists macha_private_link_read on realtime.messages;
create policy macha_private_link_read on realtime.messages
    for select to authenticated
    using (
        realtime.messages.extension = 'broadcast'
        and public.qb_is_link_member(
            case
                when realtime.topic() ~ '^link:([0-9a-fA-F-]{36})$'
                then substring(realtime.topic() from '^link:([0-9a-fA-F-]{36})$')::uuid
                else null
            end
        )
    );

drop policy if exists macha_private_link_write on realtime.messages;
create policy macha_private_link_write on realtime.messages
    for insert to authenticated
    with check (
        realtime.messages.extension = 'broadcast'
        and public.qb_is_link_member(
            case
                when realtime.topic() ~ '^link:([0-9a-fA-F-]{36})$'
                then substring(realtime.topic() from '^link:([0-9a-fA-F-]{36})$')::uuid
                else null
            end
        )
    );

select 'qb setup ok' as status;
