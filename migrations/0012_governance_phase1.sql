DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'proposal_type') THEN
    CREATE TYPE proposal_type AS ENUM ('feature', 'architecture', 'priority', 'resource', 'governance', 'bugfix');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'proposal_status') THEN
    CREATE TYPE proposal_status AS ENUM ('draft', 'open', 'voting', 'approved', 'rejected', 'vetoed', 'archived');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'author_type') THEN
    CREATE TYPE author_type AS ENUM ('human', 'ai');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'voting_rule') THEN
    CREATE TYPE voting_rule AS ENUM ('simple_majority', 'absolute_majority', 'consensus');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'cycle_template') THEN
    CREATE TYPE cycle_template AS ENUM ('fast', 'standard', 'critical');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'vote_choice') THEN
    CREATE TYPE vote_choice AS ENUM ('yes', 'no', 'abstain');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'decision_result') THEN
    CREATE TYPE decision_result AS ENUM ('approved', 'rejected', 'vetoed');
  END IF;
END $$;

CREATE TABLE IF NOT EXISTS proposals (
  id text PRIMARY KEY,
  title varchar(200) NOT NULL,
  proposal_type proposal_type NOT NULL,
  status proposal_status NOT NULL DEFAULT 'draft',
  author_id text NOT NULL,
  author_type author_type NOT NULL,
  content text NOT NULL,
  domains jsonb NOT NULL DEFAULT '[]'::jsonb,
  voting_rule voting_rule NOT NULL DEFAULT 'simple_majority',
  cycle_template cycle_template NOT NULL DEFAULT 'fast',
  created_at timestamptz NOT NULL DEFAULT now(),
  submitted_at timestamptz,
  voting_started_at timestamptz,
  voting_ended_at timestamptz,
  archived_at timestamptz
);

CREATE INDEX IF NOT EXISTS idx_proposals_status ON proposals(status);
CREATE INDEX IF NOT EXISTS idx_proposals_author ON proposals(author_id);
CREATE INDEX IF NOT EXISTS idx_proposals_created ON proposals(created_at DESC);

CREATE TABLE IF NOT EXISTS proposal_comments (
  id bigserial PRIMARY KEY,
  proposal_id text NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
  author_id text NOT NULL,
  author_type author_type NOT NULL,
  comment_type varchar(20) NOT NULL DEFAULT 'general',
  content text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_proposal_comments_proposal ON proposal_comments(proposal_id, created_at DESC);

CREATE TABLE IF NOT EXISTS votes (
  id bigserial PRIMARY KEY,
  proposal_id text NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
  voter_id text NOT NULL,
  voter_type author_type NOT NULL,
  choice vote_choice NOT NULL,
  weight double precision NOT NULL DEFAULT 1.0,
  reason text,
  voted_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT uq_votes_proposal_voter UNIQUE (proposal_id, voter_id)
);

CREATE INDEX IF NOT EXISTS idx_votes_proposal ON votes(proposal_id);

CREATE TABLE IF NOT EXISTS decisions (
  id text PRIMARY KEY,
  proposal_id text NOT NULL UNIQUE REFERENCES proposals(id) ON DELETE CASCADE,
  result decision_result NOT NULL,
  approval_rate double precision,
  total_votes integer NOT NULL DEFAULT 0,
  yes_votes integer NOT NULL DEFAULT 0,
  no_votes integer NOT NULL DEFAULT 0,
  abstain_votes integer NOT NULL DEFAULT 0,
  decided_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_decisions_decided_at ON decisions(decided_at DESC);

CREATE TABLE IF NOT EXISTS proposal_issue_links (
  id bigserial PRIMARY KEY,
  proposal_id text NOT NULL REFERENCES proposals(id) ON DELETE CASCADE,
  issue_id uuid NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  created_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT uq_proposal_issue_link UNIQUE (proposal_id, issue_id)
);

CREATE INDEX IF NOT EXISTS idx_pil_proposal ON proposal_issue_links(proposal_id);
CREATE INDEX IF NOT EXISTS idx_pil_issue ON proposal_issue_links(issue_id);
