-- Persistent safety interlock used while rolling the application back to v0.7.
-- The older binary ignores this forward-compatible table; v0.8 writers consult it before the
-- three operations that the release contract requires operators to pause during rollback.
CREATE TABLE IF NOT EXISTS flow_v08_rollback_control (
  singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
  compaction_paused BOOLEAN NOT NULL DEFAULT false,
  retention_paused BOOLEAN NOT NULL DEFAULT false,
  import_promotion_paused BOOLEAN NOT NULL DEFAULT false,
  reason TEXT,
  changed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_v08_rollback_control_reason_check CHECK (
    (compaction_paused OR retention_paused OR import_promotion_paused)
      OR reason IS NULL
  )
);

INSERT INTO flow_v08_rollback_control(singleton) VALUES(true)
ON CONFLICT (singleton) DO NOTHING;

COMMENT ON TABLE flow_v08_rollback_control IS
  'Durable rollback interlock: pause compaction, retention and package import promotion before starting a v0.7 application';
