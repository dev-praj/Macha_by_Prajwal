-- Recover an account when every device is revoked or left pending. Registration
-- is serialized per account; any remaining approved peer still has to approve
-- the re-enrolling device. This mirrors first-device enrollment.
create or replace function public.register_device(p_type text, p_name text, p_pubkey text)
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
  perform pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended(v_uid::text, 0));
  select d.id, d.status into v_id, v_status from public.remote_devices d
    where d.user_id = v_uid and d.pubkey = p_pubkey for update;
  if v_id is null then
    v_status := case when exists (select 1 from public.remote_devices d where d.user_id = v_uid and d.status = 'approved') then 'pending' else 'approved' end;
    insert into public.remote_devices(user_id, type, name, pubkey, status, last_seen)
      values (v_uid, p_type, trim(p_name), p_pubkey, v_status, now()) returning id into v_id;
  else
    if v_status in ('revoked', 'pending') then
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

revoke all on function public.register_device(text,text,text) from public, anon;
grant execute on function public.register_device(text,text,text) to authenticated;
notify pgrst, 'reload schema';
