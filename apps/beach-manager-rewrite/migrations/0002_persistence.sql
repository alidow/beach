-- persistence tables for controller leases and action log
create table if not exists controller_leases (
  lease_id text primary key,
  host_session_id text not null,
  controller_session_id text not null,
  expires_at timestamptz not null
);

create index if not exists idx_controller_leases_host on controller_leases(host_session_id);

create table if not exists action_logs (
  id text primary key,
  host_session_id text not null,
  controller_session_id text not null,
  action_type text not null,
  payload jsonb not null,
  emitted_at timestamptz not null
);

create index if not exists idx_action_logs_host on action_logs(host_session_id);
