do $$
begin
  if exists (
    select 1
    from public.copilot_accounts
    group by github_user_id
    having count(distinct supabase_user_id) > 1
  ) then
    raise exception 'A GitHub account is linked to multiple Macha users; resolve duplicate Copilot links before applying this migration.';
  end if;
end;
$$;

create unique index if not exists copilot_accounts_github_user_id_uidx
  on public.copilot_accounts (github_user_id);
