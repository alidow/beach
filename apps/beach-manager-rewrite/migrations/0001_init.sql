-- manager assignment schema
create table if not exists manager_instances (
  id text primary key,
  capacity integer not null,
  load integer not null,
  heartbeat_at timestamptz not null
);

create table if not exists manager_assignments (
  host_session_id text primary key,
  manager_instance_id text not null,
  assigned_at timestamptz not null,
  reassigned_from text
);

create index if not exists idx_manager_assignments_instance
  on manager_assignments(manager_instance_id);
