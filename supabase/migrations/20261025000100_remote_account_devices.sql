-- Account-scoped Remote mode. The earlier self-hosted QR setup uses
-- public.devices/public.queued_messages with a different schema. Keep that
-- legacy data intact and use separately named, API-visible Remote tables.
-- Run this whole migration for account-based Remote mode; setup.sql is only
-- for the hidden legacy bring-your-own-project QR flow.

create table if not exists public.remote_devices (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  type text not null check (type in ('laptop', 'phone')),
  name text not null check (length(name) between 1 and 80),
  pubkey text not null,
  status text not null default 'pending' check (status in ('pending', 'approved', 'revoked')),
  push_token text,
  created_at timestamptz not null default now(),
  last_seen timestamptz,
  unique (user_id, pubkey)
);

create table if not exists public.remote_queued_messages (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  from_device uuid not null references public.remote_devices(id) on delete cascade,
  to_device uuid not null references public.remote_devices(id) on delete cascade,
  ciphertext text not null,
  nonce text not null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null
);

create index if not exists remote_devices_user_status_idx on public.remote_devices(user_id, status);
create index if not exists remote_queue_recipient_expiry_idx on public.remote_queued_messages(to_device, expires_at);

alter table public.remote_devices enable row level security;
alter table public.remote_queued_messages enable row level security;
revoke all on public.remote_devices, public.remote_queued_messages from public, anon, authenticated;
grant select on public.remote_devices to authenticated;
grant select, insert, delete on public.remote_queued_messages to authenticated;

drop policy if exists remote_devices_select_own on public.remote_devices;
create policy remote_devices_select_own on public.remote_devices for select to authenticated
  using (user_id = (select auth.uid()));

drop policy if exists remote_queue_insert_approved_pair on public.remote_queued_messages;
create policy remote_queue_insert_approved_pair on public.remote_queued_messages for insert to authenticated
  with check (
    user_id = (select auth.uid())
    and exists (select 1 from public.remote_devices d where d.id = from_device and d.user_id = (select auth.uid()) and d.status = 'approved')
    and exists (select 1 from public.remote_devices d where d.id = to_device and d.user_id = (select auth.uid()) and d.status = 'approved')
  );
drop policy if exists remote_queue_select_recipient on public.remote_queued_messages;
create policy remote_queue_select_recipient on public.remote_queued_messages for select to authenticated
  using (
    user_id = (select auth.uid())
    and exists (select 1 from public.remote_devices d where d.id = to_device and d.user_id = (select auth.uid()) and d.status = 'approved')
  );
drop policy if exists remote_queue_delete_recipient on public.remote_queued_messages;
create policy remote_queue_delete_recipient on public.remote_queued_messages for delete to authenticated
  using (
    user_id = (select auth.uid())
    and exists (select 1 from public.remote_devices d where d.id = to_device and d.user_id = (select auth.uid()) and d.status = 'approved')
  );

-- Drop/recreate this exact signature so a previously deployed version with
-- differently named PostgREST arguments is repaired on migration re-run.
drop function if exists public.register_device(text, text, text);
create function public.register_device(p_type text, p_name text, p_pubkey text)
returns jsonb language plpgsql security definer set search_path = '' as $$
declare
  v_uid uuid := auth.uid();
  v_id uuid;
  v_status text;
begin
  if v_uid is null then raise exception 'authentication required' using errcode = '28000'; end if;
  if p_type not in ('laptop', 'phone') or p_name is null or length(trim(p_name)) not between 1 and 80
     or p_pubkey is null or length(p_pubkey) not between 32 and 256 then
    raise exception 'invalid device registration' using errcode = '22023';
  end if;
  -- Serialize first-device registration so two simultaneous new installs cannot both auto-approve.
  perform pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(v_uid::text, 0));
  select d.id, d.status into v_id, v_status from public.remote_devices d
    where d.user_id = v_uid and d.pubkey = p_pubkey for update;
  if v_id is null then
    v_status := case when exists (select 1 from public.remote_devices d where d.user_id = v_uid and d.status = 'approved') then 'pending' else 'approved' end;
    insert into public.remote_devices(user_id, type, name, pubkey, status, last_seen)
      values (v_uid, p_type, trim(p_name), p_pubkey, v_status, now()) returning id into v_id;
  else
    -- A revoked installation must be explicitly re-enrolled after a fresh
    -- Google sign-in. It becomes pending when another approved device exists,
    -- so the existing device approval flow still controls access.
    if v_status = 'revoked' then
      v_status := case when exists (
        select 1 from public.remote_devices d
        where d.user_id = v_uid and d.id <> v_id and d.status = 'approved'
      ) then 'pending' else 'approved' end;
    end if;
    update public.remote_devices set type = p_type, name = trim(p_name), status = v_status, last_seen = now()
      where id = v_id and user_id = v_uid;
  end if;
  return jsonb_build_object('device_id', v_id, 'status', v_status);
end;
$$;

create or replace function public.list_devices()
returns setof jsonb language plpgsql security definer set search_path = '' as $$
begin
  if auth.uid() is null then raise exception 'authentication required' using errcode = '28000'; end if;
  return query select jsonb_build_object('id', d.id, 'type', d.type, 'name', d.name, 'pubkey', d.pubkey,
      'status', d.status, 'created_at', d.created_at, 'last_seen', d.last_seen)
    from public.remote_devices d where d.user_id = auth.uid() order by d.created_at;
end;
$$;

create or replace function public.approve_device(p_device_id uuid, p_approver_device_id uuid)
returns boolean language plpgsql security definer set search_path = '' as $$
begin
  if auth.uid() is null then raise exception 'authentication required' using errcode = '28000'; end if;
  if not exists (select 1 from public.remote_devices a where a.id = p_approver_device_id and a.user_id = auth.uid() and a.status = 'approved') then
    raise exception 'approver device is not approved' using errcode = '42501';
  end if;
  update public.remote_devices d set status = 'approved'
    where d.id = p_device_id and d.user_id = auth.uid() and d.status = 'pending';
  return found;
end;
$$;

create or replace function public.revoke_device(p_device_id uuid)
returns boolean language plpgsql security definer set search_path = '' as $$
begin
  if auth.uid() is null then raise exception 'authentication required' using errcode = '28000'; end if;
  delete from public.remote_queued_messages q where q.user_id = auth.uid() and (q.from_device = p_device_id or q.to_device = p_device_id);
  update public.remote_devices d set status = 'revoked' where d.id = p_device_id and d.user_id = auth.uid() and d.status <> 'revoked';
  return found;
end;
$$;

create or replace function public.touch_device(p_device_id uuid)
returns boolean language plpgsql security definer set search_path = '' as $$
begin
  if auth.uid() is null then raise exception 'authentication required' using errcode = '28000'; end if;
  update public.remote_devices d set last_seen = now() where d.id = p_device_id and d.user_id = auth.uid() and d.status = 'approved';
  return found;
end;
$$;

create or replace function public.delete_my_account_data()
returns boolean language plpgsql security definer set search_path = '' as $$
declare v_uid uuid := auth.uid();
begin
  if v_uid is null then raise exception 'authentication required' using errcode = '28000'; end if;
  delete from public.remote_queued_messages where user_id = v_uid;
  delete from public.remote_devices where user_id = v_uid;
  delete from auth.users where id = v_uid;
  return found;
end;
$$;

create or replace function public.cleanup()
returns integer language plpgsql security definer set search_path = '' as $$
declare v_count integer;
begin
  if auth.uid() is null then raise exception 'authentication required' using errcode = '28000'; end if;
  delete from public.remote_queued_messages where expires_at <= now();
  get diagnostics v_count = row_count;
  return v_count;
end;
$$;

revoke all on function public.register_device(text,text,text), public.list_devices(), public.approve_device(uuid,uuid),
  public.revoke_device(uuid), public.touch_device(uuid), public.delete_my_account_data(), public.cleanup() from public, anon;
grant execute on function public.register_device(text,text,text), public.list_devices(), public.approve_device(uuid,uuid),
  public.revoke_device(uuid), public.touch_device(uuid), public.delete_my_account_data(), public.cleanup() to authenticated;

-- Private Broadcast is a wake-up hint only; message bodies are delivered via the RLS-protected queue.
-- Supabase refreshes Realtime authorization on JWT refresh/reconnect, so queue RPCs additionally verify
-- current approved status on every operation and revocation deletes queued traffic immediately.
-- Supabase owns realtime.messages and already manages/enables RLS for it.
-- Do not ALTER that managed table here; create only the app's policies below.
drop policy if exists remote_account_broadcast_read on realtime.messages;
create policy remote_account_broadcast_read on realtime.messages for select to authenticated
  using (
    (select realtime.topic()) = 'user:' || (select auth.uid())::text
    and exists (select 1 from public.remote_devices d where d.user_id = (select auth.uid()) and d.status = 'approved')
  );
drop policy if exists remote_account_broadcast_write on realtime.messages;
create policy remote_account_broadcast_write on realtime.messages for insert to authenticated
  with check (
    (select realtime.topic()) = 'user:' || (select auth.uid())::text
    and exists (select 1 from public.remote_devices d where d.user_id = (select auth.uid()) and d.status = 'approved')
  );

-- Make the newly created RPC signatures visible to PostgREST immediately.
notify pgrst, 'reload schema';

select 'setup ok' as status;
