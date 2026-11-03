-- Scope account-private Realtime authorization to Broadcast messages only.
-- realtime.messages is Supabase-managed: do not ALTER the table or change its RLS.
drop policy if exists remote_account_broadcast_read on realtime.messages;
create policy remote_account_broadcast_read on realtime.messages for select to authenticated
  using (
    extension = 'broadcast'
    and (select realtime.topic()) = 'user:' || (select auth.uid())::text
    and exists (
      select 1 from public.remote_devices d
      where d.user_id = (select auth.uid()) and d.status = 'approved'
    )
  );

drop policy if exists remote_account_broadcast_write on realtime.messages;
create policy remote_account_broadcast_write on realtime.messages for insert to authenticated
  with check (
    extension = 'broadcast'
    and (select realtime.topic()) = 'user:' || (select auth.uid())::text
    and exists (
      select 1 from public.remote_devices d
      where d.user_id = (select auth.uid()) and d.status = 'approved'
    )
  );

select 'setup ok' as status;
