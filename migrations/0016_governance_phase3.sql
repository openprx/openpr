DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'review_status') THEN
    CREATE TYPE review_status AS ENUM ('pending', 'collecting', 'completed', 'skipped');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'review_rating') THEN
    CREATE TYPE review_rating AS ENUM ('S', 'A', 'B', 'C', 'F');
  END IF;
END $$;

CREATE TABLE IF NOT EXISTS impact_reviews (
  id text PRIMARY KEY,
  proposal_id text NOT NULL UNIQUE REFERENCES proposals(id) ON DELETE CASCADE,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  status review_status NOT NULL DEFAULT 'pending',
  rating review_rating,
  metrics jsonb,
  goal_achievements json,
  achievements text,
  lessons text,
  reviewer_id uuid REFERENCES users(id) ON DELETE SET NULL,
  is_auto_triggered boolean NOT NULL DEFAULT true,
  data_sources jsonb,
  trust_score_applied boolean NOT NULL DEFAULT false,
  scheduled_at timestamptz,
  conducted_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE impact_reviews
  ADD COLUMN IF NOT EXISTS project_id uuid,
  ADD COLUMN IF NOT EXISTS metrics jsonb,
  ADD COLUMN IF NOT EXISTS goal_achievements json,
  ADD COLUMN IF NOT EXISTS is_auto_triggered boolean NOT NULL DEFAULT true;

DO $$
BEGIN
  IF EXISTS (
    SELECT 1 FROM information_schema.columns
    WHERE table_schema = 'public' AND table_name = 'impact_reviews' AND column_name = 'metrics_snapshot'
  ) THEN
    EXECUTE '
      UPDATE impact_reviews
      SET metrics = metrics_snapshot
      WHERE metrics IS NULL AND metrics_snapshot IS NOT NULL
    ';
  END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_impact_reviews_status ON impact_reviews(status);
CREATE INDEX IF NOT EXISTS idx_impact_reviews_project_id ON impact_reviews(project_id);
CREATE INDEX IF NOT EXISTS idx_impact_reviews_conducted_at ON impact_reviews(conducted_at DESC);
CREATE INDEX IF NOT EXISTS idx_impact_reviews_reviewer_id ON impact_reviews(reviewer_id);

CREATE TABLE IF NOT EXISTS impact_metrics (
  id bigserial PRIMARY KEY,
  review_id text NOT NULL REFERENCES impact_reviews(id) ON DELETE CASCADE,
  metric_key varchar(100) NOT NULL,
  metric_label varchar(150),
  target_value double precision,
  actual_value double precision,
  unit varchar(20),
  achievement_rate double precision,
  metadata jsonb,
  recorded_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT uq_impact_metrics_review_metric UNIQUE (review_id, metric_key)
);

CREATE INDEX IF NOT EXISTS idx_impact_metrics_review_id ON impact_metrics(review_id);
CREATE INDEX IF NOT EXISTS idx_impact_metrics_metric_key ON impact_metrics(metric_key);

CREATE TABLE IF NOT EXISTS review_participants (
  id bigserial PRIMARY KEY,
  review_id text NOT NULL REFERENCES impact_reviews(id) ON DELETE CASCADE,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role varchar(30) NOT NULL,
  vote_choice varchar(10),
  exercised_veto boolean NOT NULL DEFAULT false,
  veto_overturned boolean NOT NULL DEFAULT false,
  feedback_submitted boolean NOT NULL DEFAULT false,
  feedback_content text,
  trust_score_change integer,
  CONSTRAINT uq_review_participants_review_user UNIQUE (review_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_review_participants_review_id ON review_participants(review_id);
CREATE INDEX IF NOT EXISTS idx_review_participants_user_id ON review_participants(user_id);

CREATE TABLE IF NOT EXISTS ai_learning_records (
  id bigserial PRIMARY KEY,
  ai_participant_id varchar(100) NOT NULL REFERENCES ai_participants(id) ON DELETE CASCADE,
  review_id text NOT NULL REFERENCES impact_reviews(id) ON DELETE CASCADE,
  proposal_id text NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
  domain varchar(50) NOT NULL,
  review_rating review_rating NOT NULL,
  ai_vote_choice varchar(10),
  ai_vote_reason text,
  outcome_alignment varchar(20) NOT NULL,
  lesson_learned text,
  will_change text,
  follow_up_improvement varchar(20),
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_ai_learning_records_ai_participant_id ON ai_learning_records(ai_participant_id);
CREATE INDEX IF NOT EXISTS idx_ai_learning_records_review_id ON ai_learning_records(review_id);
CREATE INDEX IF NOT EXISTS idx_ai_learning_records_proposal_id ON ai_learning_records(proposal_id);

CREATE TABLE IF NOT EXISTS decision_audit_reports (
  id varchar(30) PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  period_start date NOT NULL,
  period_end date NOT NULL,
  total_proposals integer NOT NULL DEFAULT 0,
  approved_proposals integer NOT NULL DEFAULT 0,
  rejected_proposals integer NOT NULL DEFAULT 0,
  vetoed_proposals integer NOT NULL DEFAULT 0,
  reviewed_proposals integer NOT NULL DEFAULT 0,
  avg_review_rating double precision,
  rating_distribution json NOT NULL,
  top_contributors json,
  domain_stats json,
  ai_participation_stats json,
  key_insights json,
  generated_at timestamptz NOT NULL DEFAULT now(),
  generated_by varchar(30) NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_decision_audit_reports_project_id ON decision_audit_reports(project_id);
CREATE INDEX IF NOT EXISTS idx_decision_audit_reports_period ON decision_audit_reports(period_start, period_end);

CREATE TABLE IF NOT EXISTS feedback_loop_links (
  id bigserial PRIMARY KEY,
  source_review_id text NOT NULL REFERENCES impact_reviews(id) ON DELETE CASCADE,
  derived_proposal_id text NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
  link_type varchar(30) NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_feedback_loop_links_source_review_id ON feedback_loop_links(source_review_id);
CREATE INDEX IF NOT EXISTS idx_feedback_loop_links_derived_proposal_id ON feedback_loop_links(derived_proposal_id);

CREATE TABLE IF NOT EXISTS proposal_templates (
  id text PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name varchar(120) NOT NULL,
  description text,
  template_type varchar(30) NOT NULL DEFAULT 'governance',
  content jsonb NOT NULL,
  is_default boolean NOT NULL DEFAULT false,
  is_active boolean NOT NULL DEFAULT true,
  created_by uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT uq_proposal_templates_project_name UNIQUE (project_id, name)
);

CREATE INDEX IF NOT EXISTS idx_proposal_templates_project_active
  ON proposal_templates(project_id, is_active);
CREATE INDEX IF NOT EXISTS idx_proposal_templates_type ON proposal_templates(template_type);

CREATE TABLE IF NOT EXISTS governance_configs (
  id bigserial PRIMARY KEY,
  project_id uuid NOT NULL UNIQUE REFERENCES projects(id) ON DELETE CASCADE,
  review_required boolean NOT NULL DEFAULT true,
  auto_review_days integer NOT NULL DEFAULT 30,
  review_reminder_days integer NOT NULL DEFAULT 7,
  audit_report_cron varchar(100) NOT NULL DEFAULT '0 0 1 * *',
  trust_update_mode varchar(30) NOT NULL DEFAULT 'review_based',
  config jsonb NOT NULL DEFAULT '{}'::jsonb,
  updated_by uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_governance_configs_updated_at ON governance_configs(updated_at DESC);

CREATE TABLE IF NOT EXISTS governance_audit_logs (
  id bigserial PRIMARY KEY,
  project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  actor_id uuid REFERENCES users(id) ON DELETE SET NULL,
  action varchar(50) NOT NULL,
  resource_type varchar(50) NOT NULL,
  resource_id varchar(100),
  old_value jsonb,
  new_value jsonb,
  metadata jsonb,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_governance_audit_logs_project_created_at
  ON governance_audit_logs(project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_governance_audit_logs_actor_id ON governance_audit_logs(actor_id);
CREATE INDEX IF NOT EXISTS idx_governance_audit_logs_action ON governance_audit_logs(action);
