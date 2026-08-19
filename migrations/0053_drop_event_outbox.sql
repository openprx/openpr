-- The outbox delivery loop was retired with the connectors it fed, leaving this relation
-- write-only. Business events remain readable through business_events and events.tail.
DROP TABLE IF EXISTS event_outbox;
