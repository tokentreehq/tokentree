-- SPDX-License-Identifier: Apache-2.0
PRAGMA foreign_keys = ON;

CREATE TABLE schema_metadata (
  schema_version INTEGER PRIMARY KEY,
  application_version TEXT NOT NULL,
  migration_state TEXT NOT NULL CHECK (migration_state IN ('applying', 'applied', 'failed')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE projects (
  id TEXT PRIMARY KEY,
  key TEXT NOT NULL UNIQUE,
  display_name TEXT NOT NULL,
  description TEXT,
  identity_hash TEXT NOT NULL UNIQUE,
  detection_method TEXT NOT NULL,
  confidence REAL CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
  verified_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE project_aliases (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  alias_type TEXT NOT NULL,
  alias_value TEXT NOT NULL,
  verified INTEGER NOT NULL DEFAULT 0 CHECK (verified IN (0, 1)),
  PRIMARY KEY (project_id, alias_type, alias_value)
) STRICT;
CREATE UNIQUE INDEX project_alias_identity ON project_aliases(alias_type, alias_value);

CREATE TABLE project_roots (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  canonical_path TEXT NOT NULL,
  root_type TEXT NOT NULL,
  fingerprint TEXT NOT NULL,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  PRIMARY KEY (project_id, canonical_path)
) STRICT;
CREATE INDEX project_roots_fingerprint ON project_roots(fingerprint);

CREATE TABLE work_items (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  parent_id TEXT REFERENCES work_items(id) ON DELETE SET NULL,
  type TEXT NOT NULL,
  title TEXT NOT NULL,
  description TEXT,
  external_id TEXT,
  status TEXT NOT NULL,
  confidence REAL CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
  classifier_version TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  verified_at TEXT,
  CHECK (parent_id IS NULL OR parent_id <> id)
) STRICT;
CREATE INDEX work_items_project_parent ON work_items(project_id, parent_id);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  adapter TEXT NOT NULL,
  provider_session_id TEXT,
  root_session_id TEXT,
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  source_path TEXT,
  cwd TEXT,
  git_branch TEXT,
  git_commit TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT
) STRICT;
CREATE UNIQUE INDEX sessions_provider_identity ON sessions(adapter, provider_session_id) WHERE provider_session_id IS NOT NULL;

CREATE TABLE turns (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  sequence_number INTEGER NOT NULL CHECK (sequence_number >= 0),
  started_at TEXT NOT NULL,
  ended_at TEXT,
  prompt_fingerprint TEXT,
  derived_label TEXT,
  prompt_storage_mode TEXT NOT NULL DEFAULT 'fingerprint_only' CHECK (prompt_storage_mode IN ('none', 'fingerprint_only', 'redacted_label')),
  UNIQUE (session_id, sequence_number)
) STRICT;

CREATE TABLE usage_events (
  id TEXT PRIMARY KEY,
  adapter TEXT NOT NULL,
  source_kind TEXT NOT NULL,
  source_event_id TEXT,
  source_process_id TEXT,
  source_sequence INTEGER,
  session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
  prompt_id TEXT,
  turn_id TEXT REFERENCES turns(id) ON DELETE SET NULL,
  request_id TEXT,
  agent_id TEXT,
  parent_agent_id TEXT,
  parent_event_id TEXT REFERENCES usage_events(id) ON DELETE SET NULL,
  source_timestamp TEXT,
  observed_at TEXT NOT NULL,
  ingested_at TEXT NOT NULL,
  model TEXT,
  service_tier TEXT,
  region TEXT,
  input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
  cached_input_tokens INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
  cache_write_tokens INTEGER CHECK (cache_write_tokens IS NULL OR cache_write_tokens >= 0),
  output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
  reasoning_tokens INTEGER CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
  provider_reported_cost_micros INTEGER CHECK (provider_reported_cost_micros IS NULL OR provider_reported_cost_micros >= 0),
  source_path TEXT,
  source_offset INTEGER CHECK (source_offset IS NULL OR source_offset >= 0),
  event_hash TEXT NOT NULL UNIQUE,
  adapter_version TEXT NOT NULL,
  parser_version TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX usage_events_source_identity ON usage_events(adapter, source_kind, source_event_id) WHERE source_event_id IS NOT NULL;
CREATE INDEX usage_events_request ON usage_events(request_id) WHERE request_id IS NOT NULL;
CREATE INDEX usage_events_session_time ON usage_events(session_id, source_timestamp);

CREATE TRIGGER usage_events_no_update BEFORE UPDATE ON usage_events
BEGIN SELECT RAISE(ABORT, 'usage_events are append-only'); END;
CREATE TRIGGER usage_events_no_delete BEFORE DELETE ON usage_events
BEGIN SELECT RAISE(ABORT, 'usage_events are append-only'); END;

CREATE TABLE usage_spans (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  start_turn_id TEXT REFERENCES turns(id) ON DELETE SET NULL,
  end_turn_id TEXT REFERENCES turns(id) ON DELETE SET NULL,
  measurement_status TEXT NOT NULL CHECK (measurement_status IN ('measured', 'partial', 'unavailable', 'anomalous')),
  measured_usage_json TEXT,
  completeness REAL CHECK (completeness IS NULL OR (completeness >= 0 AND completeness <= 100))
) STRICT;

CREATE TABLE attribution_groups (
  id TEXT PRIMARY KEY,
  usage_span_id TEXT NOT NULL REFERENCES usage_spans(id) ON DELETE CASCADE,
  policy TEXT NOT NULL,
  supersedes_group_id TEXT REFERENCES attribution_groups(id) ON DELETE SET NULL,
  active INTEGER NOT NULL CHECK (active IN (0, 1)),
  created_at TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX one_active_attribution_group ON attribution_groups(usage_span_id) WHERE active = 1;

CREATE TABLE attributions (
  group_id TEXT NOT NULL REFERENCES attribution_groups(id) ON DELETE CASCADE,
  project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
  work_item_id TEXT REFERENCES work_items(id) ON DELETE SET NULL,
  role TEXT NOT NULL,
  weight_basis_points INTEGER NOT NULL CHECK (weight_basis_points BETWEEN 1 AND 10000),
  method TEXT NOT NULL,
  confidence REAL CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
  explanation TEXT,
  classifier_version TEXT,
  verified_by_user INTEGER NOT NULL DEFAULT 0 CHECK (verified_by_user IN (0, 1)),
  PRIMARY KEY (group_id, role),
  CHECK (project_id IS NOT NULL OR work_item_id IS NOT NULL)
) STRICT;

CREATE TABLE classification_events (
  id TEXT PRIMARY KEY,
  turn_id TEXT NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
  outcome TEXT NOT NULL CHECK (outcome IN ('CONTINUE', 'CHILD', 'SWITCH', 'UNCERTAIN')),
  signals_json TEXT NOT NULL,
  score REAL NOT NULL,
  classifier_version TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT;

CREATE TABLE pricing_versions (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  model_pattern TEXT NOT NULL,
  effective_from TEXT NOT NULL,
  effective_to TEXT,
  rates_json TEXT NOT NULL,
  source TEXT NOT NULL,
  retrieved_at TEXT NOT NULL,
  signature_or_hash TEXT NOT NULL
) STRICT;

CREATE TABLE cost_calculations (
  usage_event_id TEXT NOT NULL REFERENCES usage_events(id) ON DELETE RESTRICT,
  pricing_version_id TEXT REFERENCES pricing_versions(id) ON DELETE RESTRICT,
  amount_micros INTEGER CHECK (amount_micros IS NULL OR amount_micros >= 0),
  currency TEXT NOT NULL DEFAULT 'USD',
  cost_type TEXT NOT NULL CHECK (cost_type IN ('reconciled_billed_cost','provider_reported_cost','configured_rate_estimate','api_equivalent_estimate','allocated_subscription_cost','unavailable')),
  attribution_policy TEXT NOT NULL,
  coverage_json TEXT NOT NULL,
  calculated_at TEXT NOT NULL,
  PRIMARY KEY (usage_event_id, pricing_version_id, cost_type),
  CHECK ((cost_type = 'unavailable' AND amount_micros IS NULL) OR cost_type <> 'unavailable')
) STRICT;

CREATE TABLE ingestion_checkpoints (
  adapter TEXT NOT NULL,
  source_path TEXT NOT NULL,
  file_size INTEGER NOT NULL CHECK (file_size >= 0),
  modified_at TEXT NOT NULL,
  last_offset INTEGER NOT NULL CHECK (last_offset >= 0),
  last_event_hash TEXT,
  PRIMARY KEY (adapter, source_path)
) STRICT;

CREATE TABLE measurement_anomalies (
  id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
  turn_id TEXT REFERENCES turns(id) ON DELETE SET NULL,
  type TEXT NOT NULL,
  source_values_json TEXT NOT NULL,
  resolution TEXT,
  created_at TEXT NOT NULL,
  resolved_at TEXT
) STRICT;

CREATE TABLE adapter_capabilities (
  adapter TEXT NOT NULL,
  adapter_version TEXT NOT NULL,
  host_version TEXT NOT NULL,
  capability TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('available', 'degraded', 'blocked', 'unavailable', 'unknown')),
  detail TEXT,
  checked_at TEXT NOT NULL,
  PRIMARY KEY (adapter, adapter_version, host_version, capability)
) STRICT;

CREATE TABLE coverage_manifests (
  adapter TEXT NOT NULL,
  version TEXT NOT NULL,
  model_tokens TEXT NOT NULL,
  cache_tokens TEXT NOT NULL,
  reasoning_tokens TEXT NOT NULL,
  subagent_tokens TEXT NOT NULL,
  server_tools TEXT NOT NULL,
  external_costs TEXT NOT NULL,
  runtime_costs TEXT NOT NULL,
  classifier_overhead TEXT NOT NULL,
  PRIMARY KEY (adapter, version)
) STRICT;

CREATE TABLE notes (
  id TEXT PRIMARY KEY,
  project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
  work_item_id TEXT REFERENCES work_items(id) ON DELETE CASCADE,
  text TEXT NOT NULL CHECK (length(text) BETWEEN 1 AND 240),
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  CHECK (project_id IS NOT NULL OR work_item_id IS NOT NULL)
) STRICT;

CREATE INDEX anomalies_unresolved ON measurement_anomalies(session_id, turn_id) WHERE resolved_at IS NULL;
CREATE INDEX costs_type ON cost_calculations(cost_type, currency);
