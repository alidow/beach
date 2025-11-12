CREATE EXTENSION IF NOT EXISTS citext WITH SCHEMA public;

CREATE EXTENSION IF NOT EXISTS "uuid-ossp" WITH SCHEMA public;

CREATE TYPE public.account_status AS ENUM (
    'active',
    'disabled'
);

CREATE TYPE public.account_type AS ENUM (
    'human',
    'agent',
    'service'
);

CREATE TYPE public.automation_role AS ENUM (
    'observer',
    'controller',
    'coordinator'
);

CREATE TYPE public.controller_event_type AS ENUM (
    'registered',
    'lease_acquired',
    'lease_released',
    'actions_queued',
    'actions_acked',
    'health_reported',
    'state_updated',
    'pairing_added',
    'pairing_removed'
);

CREATE TYPE public.controller_update_cadence AS ENUM (
    'fast',
    'balanced',
    'slow'
);

CREATE TYPE public.group_role AS ENUM (
    'admin',
    'member'
);

CREATE TYPE public.harness_type AS ENUM (
    'terminal_shim',
    'cabana_adapter',
    'remote_widget',
    'service_proxy',
    'custom'
);

CREATE TYPE public.membership_role AS ENUM (
    'owner',
    'admin',
    'contributor',
    'viewer'
);

CREATE TYPE public.membership_status AS ENUM (
    'active',
    'invited',
    'suspended',
    'revoked'
);

CREATE TYPE public.organization_role AS ENUM (
    'owner',
    'admin',
    'billing',
    'member'
);

CREATE TYPE public.session_kind AS ENUM (
    'terminal',
    'cabana_gui',
    'manager_console',
    'widget',
    'spectator_feed',
    'service_daemon'
);

CREATE TABLE public.account (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    type public.account_type NOT NULL,
    status public.account_status DEFAULT 'active'::public.account_status NOT NULL,
    beach_gate_subject text NOT NULL,
    display_name text,
    email public.citext,
    avatar_url text,
    metadata jsonb DEFAULT '{}'::jsonb,
    default_organization_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.automation_assignment (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    private_beach_id uuid NOT NULL,
    controller_account_id uuid NOT NULL,
    role public.automation_role NOT NULL,
    session_id uuid,
    config jsonb DEFAULT '{}'::jsonb,
    created_by_account_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.beach_group (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    private_beach_id uuid NOT NULL,
    name text NOT NULL,
    description text,
    created_by_account_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.controller_event (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    session_id uuid NOT NULL,
    event_type public.controller_event_type NOT NULL,
    controller_token uuid,
    reason text,
    occurred_at timestamp with time zone DEFAULT now() NOT NULL,
    controller_account_id uuid,
    issued_by_account_id uuid,
    controller_token_id uuid
);

CREATE TABLE public.controller_lease (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    session_id uuid NOT NULL,
    controller_account_id uuid,
    issued_by_account_id uuid,
    reason text,
    issued_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone
);

CREATE TABLE public.controller_pairing (
    controller_session_id uuid NOT NULL,
    child_session_id uuid NOT NULL,
    prompt_template text,
    update_cadence public.controller_update_cadence DEFAULT 'balanced'::public.controller_update_cadence NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.file_record (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    private_beach_id uuid NOT NULL,
    path text NOT NULL,
    version integer DEFAULT 1 NOT NULL,
    storage_key text NOT NULL,
    size_bytes bigint,
    content_type text,
    checksum text,
    uploaded_by_account_id uuid,
    uploaded_by_session_id uuid,
    uploaded_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.group_membership (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    beach_group_id uuid NOT NULL,
    account_id uuid NOT NULL,
    role public.group_role NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.organization (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    name text NOT NULL,
    slug public.citext NOT NULL,
    billing_email public.citext,
    metadata jsonb DEFAULT '{}'::jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    archived_at timestamp with time zone
);

CREATE TABLE public.organization_membership (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    organization_id uuid NOT NULL,
    account_id uuid NOT NULL,
    role public.organization_role NOT NULL,
    created_by_account_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.private_beach (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    organization_id uuid,
    owner_account_id uuid,
    name text NOT NULL,
    slug public.citext NOT NULL,
    description text,
    default_role public.membership_role DEFAULT 'viewer'::public.membership_role NOT NULL,
    layout_preset jsonb DEFAULT '{}'::jsonb,
    settings jsonb DEFAULT '{}'::jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    archived_at timestamp with time zone
);

CREATE TABLE public.private_beach_membership (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    private_beach_id uuid NOT NULL,
    account_id uuid NOT NULL,
    role public.membership_role NOT NULL,
    status public.membership_status DEFAULT 'active'::public.membership_status NOT NULL,
    invited_by_account_id uuid,
    invitation_token_hash text,
    invited_at timestamp with time zone,
    activated_at timestamp with time zone,
    suspended_at timestamp with time zone,
    notes text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.session (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    private_beach_id uuid NOT NULL,
    origin_session_id uuid NOT NULL,
    harness_id uuid,
    kind public.session_kind NOT NULL,
    title text,
    display_order integer,
    location_hint text,
    capabilities jsonb DEFAULT '[]'::jsonb,
    metadata jsonb DEFAULT '{}'::jsonb,
    created_by_account_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone,
    ended_at timestamp with time zone,
    harness_type public.harness_type,
    attach_method text,
    CONSTRAINT session_attach_method_check CHECK ((attach_method = ANY (ARRAY['code'::text, 'owned'::text, 'direct'::text])))
);

CREATE TABLE public.session_runtime (
    session_id uuid NOT NULL,
    state_cache_url text,
    transport_hints jsonb DEFAULT '{}'::jsonb,
    last_health jsonb,
    last_health_at timestamp with time zone,
    viewer_passcode text
);

CREATE TABLE public.session_tag (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    session_id uuid NOT NULL,
    tag text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.share_link (
    id uuid DEFAULT public.uuid_generate_v4() NOT NULL,
    private_beach_id uuid NOT NULL,
    created_by_account_id uuid,
    label text,
    token_hash text NOT NULL,
    granted_role public.membership_role NOT NULL,
    max_uses integer,
    use_count integer DEFAULT 0 NOT NULL,
    expires_at timestamp with time zone,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.surfer_canvas_layout (
    private_beach_id uuid NOT NULL,
    layout jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

ALTER TABLE ONLY public.account
    ADD CONSTRAINT account_beach_gate_subject_key UNIQUE (beach_gate_subject);

ALTER TABLE ONLY public.account
    ADD CONSTRAINT account_email_key UNIQUE (email);

ALTER TABLE ONLY public.account
    ADD CONSTRAINT account_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.automation_assignment
    ADD CONSTRAINT automation_assignment_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.beach_group
    ADD CONSTRAINT beach_group_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.controller_event
    ADD CONSTRAINT controller_event_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.controller_lease
    ADD CONSTRAINT controller_lease_pkey PRIMARY KEY (id);


ALTER TABLE ONLY public.controller_pairing
    ADD CONSTRAINT controller_pairing_pkey PRIMARY KEY (controller_session_id, child_session_id);

ALTER TABLE ONLY public.file_record
    ADD CONSTRAINT file_record_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.file_record
    ADD CONSTRAINT file_record_private_beach_id_path_version_key UNIQUE (private_beach_id, path, version);

ALTER TABLE ONLY public.group_membership
    ADD CONSTRAINT group_membership_beach_group_id_account_id_key UNIQUE (beach_group_id, account_id);

ALTER TABLE ONLY public.group_membership
    ADD CONSTRAINT group_membership_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.organization_membership
    ADD CONSTRAINT organization_membership_organization_id_account_id_key UNIQUE (organization_id, account_id);

ALTER TABLE ONLY public.organization_membership
    ADD CONSTRAINT organization_membership_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.organization
    ADD CONSTRAINT organization_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.organization
    ADD CONSTRAINT organization_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.private_beach_membership
    ADD CONSTRAINT private_beach_membership_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.private_beach_membership
    ADD CONSTRAINT private_beach_membership_private_beach_id_account_id_key UNIQUE (private_beach_id, account_id);

ALTER TABLE ONLY public.private_beach
    ADD CONSTRAINT private_beach_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.private_beach
    ADD CONSTRAINT private_beach_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.session
    ADD CONSTRAINT session_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.session
    ADD CONSTRAINT session_private_beach_id_origin_session_id_key UNIQUE (private_beach_id, origin_session_id);

ALTER TABLE ONLY public.session_runtime
    ADD CONSTRAINT session_runtime_pkey PRIMARY KEY (session_id);

ALTER TABLE ONLY public.session_tag
    ADD CONSTRAINT session_tag_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.share_link
    ADD CONSTRAINT share_link_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.share_link
    ADD CONSTRAINT share_link_token_hash_key UNIQUE (token_hash);

ALTER TABLE ONLY public.surfer_canvas_layout
    ADD CONSTRAINT surfer_canvas_layout_pkey PRIMARY KEY (private_beach_id);

CREATE UNIQUE INDEX automation_assignment_unique_session ON public.automation_assignment USING btree (controller_account_id, session_id) WHERE (session_id IS NOT NULL);

CREATE UNIQUE INDEX idx_beach_group_name_ci ON public.beach_group USING btree (private_beach_id, lower(name));

CREATE INDEX idx_controller_event_session ON public.controller_event USING btree (session_id, occurred_at DESC);

CREATE INDEX idx_controller_lease_session ON public.controller_lease USING btree (session_id);

CREATE INDEX idx_controller_pairing_child ON public.controller_pairing USING btree (child_session_id);

CREATE INDEX idx_private_beach_membership_account ON public.private_beach_membership USING btree (account_id);

CREATE INDEX idx_session_private_beach ON public.session USING btree (private_beach_id);

CREATE UNIQUE INDEX idx_session_tag_ci ON public.session_tag USING btree (session_id, lower(tag));

ALTER TABLE ONLY public.account
    ADD CONSTRAINT account_default_organization_id_fkey FOREIGN KEY (default_organization_id) REFERENCES public.organization(id);

ALTER TABLE ONLY public.automation_assignment
    ADD CONSTRAINT automation_assignment_controller_account_id_fkey FOREIGN KEY (controller_account_id) REFERENCES public.account(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.automation_assignment
    ADD CONSTRAINT automation_assignment_created_by_account_id_fkey FOREIGN KEY (created_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.automation_assignment
    ADD CONSTRAINT automation_assignment_private_beach_id_fkey FOREIGN KEY (private_beach_id) REFERENCES public.private_beach(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.automation_assignment
    ADD CONSTRAINT automation_assignment_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.session(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.beach_group
    ADD CONSTRAINT beach_group_created_by_account_id_fkey FOREIGN KEY (created_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.beach_group
    ADD CONSTRAINT beach_group_private_beach_id_fkey FOREIGN KEY (private_beach_id) REFERENCES public.private_beach(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.controller_event
    ADD CONSTRAINT controller_event_controller_account_id_fkey FOREIGN KEY (controller_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.controller_event
    ADD CONSTRAINT controller_event_controller_token_id_fkey FOREIGN KEY (controller_token_id) REFERENCES public.controller_lease(id);

ALTER TABLE ONLY public.controller_event
    ADD CONSTRAINT controller_event_issued_by_account_id_fkey FOREIGN KEY (issued_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.controller_event
    ADD CONSTRAINT controller_event_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.session(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.controller_lease
    ADD CONSTRAINT controller_lease_controller_account_id_fkey FOREIGN KEY (controller_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.controller_lease
    ADD CONSTRAINT controller_lease_issued_by_account_id_fkey FOREIGN KEY (issued_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.controller_lease
    ADD CONSTRAINT controller_lease_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.session(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.controller_pairing
    ADD CONSTRAINT controller_pairing_child_session_id_fkey FOREIGN KEY (child_session_id) REFERENCES public.session(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.controller_pairing
    ADD CONSTRAINT controller_pairing_controller_session_id_fkey FOREIGN KEY (controller_session_id) REFERENCES public.session(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_record
    ADD CONSTRAINT file_record_private_beach_id_fkey FOREIGN KEY (private_beach_id) REFERENCES public.private_beach(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_record
    ADD CONSTRAINT file_record_uploaded_by_account_id_fkey FOREIGN KEY (uploaded_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.file_record
    ADD CONSTRAINT file_record_uploaded_by_session_id_fkey FOREIGN KEY (uploaded_by_session_id) REFERENCES public.session(id);

ALTER TABLE ONLY public.group_membership
    ADD CONSTRAINT group_membership_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.account(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.group_membership
    ADD CONSTRAINT group_membership_beach_group_id_fkey FOREIGN KEY (beach_group_id) REFERENCES public.beach_group(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.organization_membership
    ADD CONSTRAINT organization_membership_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.account(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.organization_membership
    ADD CONSTRAINT organization_membership_created_by_account_id_fkey FOREIGN KEY (created_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.organization_membership
    ADD CONSTRAINT organization_membership_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organization(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.private_beach_membership
    ADD CONSTRAINT private_beach_membership_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.account(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.private_beach_membership
    ADD CONSTRAINT private_beach_membership_invited_by_account_id_fkey FOREIGN KEY (invited_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.private_beach_membership
    ADD CONSTRAINT private_beach_membership_private_beach_id_fkey FOREIGN KEY (private_beach_id) REFERENCES public.private_beach(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.private_beach
    ADD CONSTRAINT private_beach_organization_id_fkey FOREIGN KEY (organization_id) REFERENCES public.organization(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.private_beach
    ADD CONSTRAINT private_beach_owner_account_id_fkey FOREIGN KEY (owner_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.session
    ADD CONSTRAINT session_created_by_account_id_fkey FOREIGN KEY (created_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.session
    ADD CONSTRAINT session_private_beach_id_fkey FOREIGN KEY (private_beach_id) REFERENCES public.private_beach(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.session_runtime
    ADD CONSTRAINT session_runtime_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.session(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.session_tag
    ADD CONSTRAINT session_tag_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.session(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.share_link
    ADD CONSTRAINT share_link_created_by_account_id_fkey FOREIGN KEY (created_by_account_id) REFERENCES public.account(id);

ALTER TABLE ONLY public.share_link
    ADD CONSTRAINT share_link_private_beach_id_fkey FOREIGN KEY (private_beach_id) REFERENCES public.private_beach(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.surfer_canvas_layout
    ADD CONSTRAINT surfer_canvas_layout_private_beach_id_fkey FOREIGN KEY (private_beach_id) REFERENCES public.private_beach(id) ON DELETE CASCADE;

ALTER TABLE public.automation_assignment ENABLE ROW LEVEL SECURITY;

CREATE POLICY automation_assignment_all ON public.automation_assignment USING (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))) WITH CHECK (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)));

ALTER TABLE public.controller_event ENABLE ROW LEVEL SECURITY;

CREATE POLICY controller_event_all ON public.controller_event USING ((EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = controller_event.session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))))));

ALTER TABLE public.controller_lease ENABLE ROW LEVEL SECURITY;

CREATE POLICY controller_lease_all ON public.controller_lease USING ((EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = controller_lease.session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)))))) WITH CHECK ((EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = controller_lease.session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))))));

ALTER TABLE public.controller_pairing ENABLE ROW LEVEL SECURITY;

CREATE POLICY controller_pairing_all ON public.controller_pairing USING (((EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = controller_pairing.controller_session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))))) AND (EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = controller_pairing.child_session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))))))) WITH CHECK (((EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = controller_pairing.controller_session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))))) AND (EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = controller_pairing.child_session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)))))));

CREATE POLICY file_record_all ON public.file_record USING (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))) WITH CHECK (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)));

ALTER TABLE public.private_beach ENABLE ROW LEVEL SECURITY;

CREATE POLICY private_beach_delete ON public.private_beach FOR DELETE USING (((id)::text = current_setting('beach.private_beach_id'::text, true)));

CREATE POLICY private_beach_insert ON public.private_beach FOR INSERT WITH CHECK (((id)::text = current_setting('beach.private_beach_id'::text, true)));

CREATE POLICY private_beach_member_select ON public.private_beach FOR SELECT USING (((EXISTS ( SELECT 1
   FROM public.private_beach_membership m
  WHERE ((m.private_beach_id = private_beach.id) AND ((m.account_id)::text = current_setting('beach.account_id'::text, true)) AND (m.status = 'active'::public.membership_status)))) OR ((owner_account_id)::text = current_setting('beach.account_id'::text, true)) OR ((owner_account_id IS NULL) AND ((id)::text = current_setting('beach.private_beach_id'::text, true)))));

ALTER TABLE public.private_beach_membership ENABLE ROW LEVEL SECURITY;

CREATE POLICY private_beach_membership_insert ON public.private_beach_membership FOR INSERT WITH CHECK ((((account_id)::text = current_setting('beach.account_id'::text, true)) AND ((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))));

CREATE POLICY private_beach_membership_select ON public.private_beach_membership FOR SELECT USING (((account_id)::text = current_setting('beach.account_id'::text, true)));

CREATE POLICY private_beach_update ON public.private_beach FOR UPDATE USING (((id)::text = current_setting('beach.private_beach_id'::text, true))) WITH CHECK (((id)::text = current_setting('beach.private_beach_id'::text, true)));

ALTER TABLE public.session ENABLE ROW LEVEL SECURITY;

CREATE POLICY session_delete ON public.session FOR DELETE USING (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)));

CREATE POLICY session_insert ON public.session FOR INSERT WITH CHECK (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)));

ALTER TABLE public.session_runtime ENABLE ROW LEVEL SECURITY;

CREATE POLICY session_runtime_all ON public.session_runtime USING ((EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = session_runtime.session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)))))) WITH CHECK ((EXISTS ( SELECT 1
   FROM public.session s
  WHERE ((s.id = session_runtime.session_id) AND ((s.private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))))));

CREATE POLICY session_select ON public.session FOR SELECT USING (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)));

CREATE POLICY session_update ON public.session FOR UPDATE USING (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))) WITH CHECK (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)));

ALTER TABLE public.surfer_canvas_layout ENABLE ROW LEVEL SECURITY;

CREATE POLICY surfer_canvas_layout_all ON public.surfer_canvas_layout USING (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true))) WITH CHECK (((private_beach_id)::text = current_setting('beach.private_beach_id'::text, true)));
