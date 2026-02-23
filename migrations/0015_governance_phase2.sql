DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'trust_level') THEN
    CREATE TYPE trust_level AS ENUM ('observer', 'advisor', 'voter', 'vetoer', 'autonomous');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'participant_type') THEN
    CREATE TYPE participant_type AS ENUM ('human', 'ai');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'veto_status') THEN
    CREATE TYPE veto_status AS ENUM ('active', 'overturned', 'upheld', 'withdrawn');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'appeal_status') THEN
    CREATE TYPE appeal_status AS ENUM ('pending', 'accepted', 'rejected');
  END IF;
END $$;

CREATE TABLE IF NOT EXISTS decision_domains (
  id varchar(50) PRIMARY KEY,
  project_id uuid NOT NULL,
  name varchar(100) NOT NULL,
  description text,
  default_voting_rule varchar(30) NOT NULL DEFAULT 'simple_majority',
  default_cycle_template varchar(20) NOT NULL DEFAULT 'fast',
  veto_threshold integer NOT NULL DEFAULT 200,
  autonomous_threshold integer NOT NULL DEFAULT 300,
  is_active boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_domains_project ON decision_domains(project_id);

CREATE TABLE IF NOT EXISTS trust_scores (
  id bigserial PRIMARY KEY,
  user_id uuid NOT NULL,
  user_type participant_type NOT NULL,
  project_id uuid NOT NULL,
  domain varchar(50) NOT NULL DEFAULT 'global',
  score integer NOT NULL DEFAULT 100,
  level trust_level NOT NULL DEFAULT 'observer',
  vote_weight double precision NOT NULL DEFAULT 1.0,
  consecutive_rejections integer NOT NULL DEFAULT 0,
  cooldown_until timestamptz,
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT uq_trust_user_project_domain UNIQUE (user_id, project_id, domain)
);

CREATE INDEX IF NOT EXISTS idx_trust_project_domain ON trust_scores(project_id, domain);

CREATE TABLE IF NOT EXISTS trust_score_logs (
  id bigserial PRIMARY KEY,
  user_id uuid NOT NULL,
  project_id uuid NOT NULL,
  domain varchar(50) NOT NULL,
  event_type varchar(50) NOT NULL,
  event_id varchar(50) NOT NULL,
  score_change integer NOT NULL,
  old_score integer NOT NULL,
  new_score integer NOT NULL,
  old_level trust_level NOT NULL,
  new_level trust_level NOT NULL,
  reason text NOT NULL,
  is_appealed boolean NOT NULL DEFAULT false,
  appeal_result varchar(20),
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_trust_score_logs_user_project_domain
  ON trust_score_logs(user_id, project_id, domain, created_at DESC);

CREATE TABLE IF NOT EXISTS ai_participants (
  id varchar(100) PRIMARY KEY,
  project_id uuid NOT NULL,
  name varchar(100) NOT NULL,
  model varchar(100) NOT NULL,
  provider varchar(50) NOT NULL,
  api_endpoint text,
  capabilities jsonb NOT NULL,
  domain_overrides jsonb,
  max_domain_level varchar(20) NOT NULL DEFAULT 'voter',
  can_veto_human_consensus boolean NOT NULL DEFAULT false,
  reason_min_length integer NOT NULL DEFAULT 50,
  is_active boolean NOT NULL DEFAULT true,
  registered_by uuid NOT NULL,
  last_active_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_ai_participants_project_active
  ON ai_participants(project_id, is_active);

CREATE TABLE IF NOT EXISTS vetoers (
  id bigserial PRIMARY KEY,
  user_id uuid NOT NULL,
  project_id uuid NOT NULL,
  domain varchar(50) NOT NULL,
  granted_by varchar(20) NOT NULL,
  granted_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT uq_vetoers UNIQUE (user_id, project_id, domain)
);

CREATE INDEX IF NOT EXISTS idx_vetoers_project_domain ON vetoers(project_id, domain);

CREATE TABLE IF NOT EXISTS veto_events (
  id bigserial PRIMARY KEY,
  proposal_id text NOT NULL,
  vetoer_id varchar(100) NOT NULL,
  domain varchar(50) NOT NULL,
  reason text NOT NULL,
  status veto_status NOT NULL DEFAULT 'active',
  escalation_started_at timestamptz,
  escalation_result varchar(20),
  escalation_votes jsonb,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_veto_events_proposal ON veto_events(proposal_id);
CREATE INDEX IF NOT EXISTS idx_veto_events_status ON veto_events(status);

ALTER TABLE decisions
  ADD COLUMN IF NOT EXISTS weighted_yes double precision,
  ADD COLUMN IF NOT EXISTS weighted_no double precision,
  ADD COLUMN IF NOT EXISTS weighted_approval_rate double precision,
  ADD COLUMN IF NOT EXISTS is_weighted boolean NOT NULL DEFAULT false,
  ADD COLUMN IF NOT EXISTS veto_event_id bigint;

CREATE INDEX IF NOT EXISTS idx_decisions_veto_event_id ON decisions(veto_event_id);

CREATE TABLE IF NOT EXISTS appeals (
  id bigserial PRIMARY KEY,
  log_id bigint NOT NULL,
  appellant_id uuid NOT NULL,
  reason text NOT NULL,
  evidence jsonb,
  status appeal_status NOT NULL DEFAULT 'pending',
  reviewer_id uuid,
  review_note text,
  created_at timestamptz NOT NULL DEFAULT now(),
  resolved_at timestamptz
);

CREATE INDEX IF NOT EXISTS idx_appeals_log_id ON appeals(log_id);
CREATE INDEX IF NOT EXISTS idx_appeals_status_created_at ON appeals(status, created_at DESC);

DO $$
BEGIN
  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'decision_domains'
      AND column_name = 'project_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE decision_domains ALTER COLUMN project_id TYPE uuid USING project_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'trust_scores'
      AND column_name = 'user_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE trust_scores ALTER COLUMN user_id TYPE uuid USING user_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'trust_scores'
      AND column_name = 'project_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE trust_scores ALTER COLUMN project_id TYPE uuid USING project_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'trust_score_logs'
      AND column_name = 'user_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE trust_score_logs ALTER COLUMN user_id TYPE uuid USING user_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'trust_score_logs'
      AND column_name = 'project_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE trust_score_logs ALTER COLUMN project_id TYPE uuid USING project_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'ai_participants'
      AND column_name = 'project_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE ai_participants ALTER COLUMN project_id TYPE uuid USING project_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'ai_participants'
      AND column_name = 'registered_by' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE ai_participants ALTER COLUMN registered_by TYPE uuid USING registered_by::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'vetoers'
      AND column_name = 'user_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE vetoers ALTER COLUMN user_id TYPE uuid USING user_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'vetoers'
      AND column_name = 'project_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE vetoers ALTER COLUMN project_id TYPE uuid USING project_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'veto_events'
      AND column_name = 'proposal_id' AND data_type = 'character varying'
  ) THEN
    EXECUTE 'ALTER TABLE veto_events ALTER COLUMN proposal_id TYPE text';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'veto_events'
      AND column_name = 'vetoer_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE veto_events ALTER COLUMN vetoer_id TYPE uuid USING vetoer_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'appeals'
      AND column_name = 'appellant_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE appeals ALTER COLUMN appellant_id TYPE uuid USING appellant_id::uuid';
  END IF;

  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'appeals'
      AND column_name = 'reviewer_id' AND data_type <> 'uuid'
  ) THEN
    EXECUTE 'ALTER TABLE appeals ALTER COLUMN reviewer_id TYPE uuid USING reviewer_id::uuid';
  END IF;
END $$;

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_decisions_veto_event_id') THEN
    ALTER TABLE decisions
      ADD CONSTRAINT fk_decisions_veto_event_id
      FOREIGN KEY (veto_event_id) REFERENCES veto_events(id) ON DELETE SET NULL;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_veto_events_proposal_id') THEN
    ALTER TABLE veto_events
      ADD CONSTRAINT fk_veto_events_proposal_id
      FOREIGN KEY (proposal_id) REFERENCES proposals(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_veto_events_vetoer_id') THEN
    ALTER TABLE veto_events
      ADD CONSTRAINT fk_veto_events_vetoer_id
      FOREIGN KEY (vetoer_id) REFERENCES users(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_appeals_log_id') THEN
    ALTER TABLE appeals
      ADD CONSTRAINT fk_appeals_log_id
      FOREIGN KEY (log_id) REFERENCES trust_score_logs(id);
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_appeals_appellant_id') THEN
    ALTER TABLE appeals
      ADD CONSTRAINT fk_appeals_appellant_id
      FOREIGN KEY (appellant_id) REFERENCES users(id);
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_appeals_reviewer_id') THEN
    ALTER TABLE appeals
      ADD CONSTRAINT fk_appeals_reviewer_id
      FOREIGN KEY (reviewer_id) REFERENCES users(id) ON DELETE SET NULL;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_decision_domains_project_id') THEN
    ALTER TABLE decision_domains
      ADD CONSTRAINT fk_decision_domains_project_id
      FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_trust_scores_user_id') THEN
    ALTER TABLE trust_scores
      ADD CONSTRAINT fk_trust_scores_user_id
      FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_trust_scores_project_id') THEN
    ALTER TABLE trust_scores
      ADD CONSTRAINT fk_trust_scores_project_id
      FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_trust_score_logs_user_id') THEN
    ALTER TABLE trust_score_logs
      ADD CONSTRAINT fk_trust_score_logs_user_id
      FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_trust_score_logs_project_id') THEN
    ALTER TABLE trust_score_logs
      ADD CONSTRAINT fk_trust_score_logs_project_id
      FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_ai_participants_project_id') THEN
    ALTER TABLE ai_participants
      ADD CONSTRAINT fk_ai_participants_project_id
      FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_ai_participants_registered_by') THEN
    ALTER TABLE ai_participants
      ADD CONSTRAINT fk_ai_participants_registered_by
      FOREIGN KEY (registered_by) REFERENCES users(id);
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_vetoers_user_id') THEN
    ALTER TABLE vetoers
      ADD CONSTRAINT fk_vetoers_user_id
      FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_vetoers_project_id') THEN
    ALTER TABLE vetoers
      ADD CONSTRAINT fk_vetoers_project_id
      FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE;
  END IF;
END $$;
