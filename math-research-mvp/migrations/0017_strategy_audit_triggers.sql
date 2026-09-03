ALTER TABLE strategy_states
ADD COLUMN trigger_reasons_json TEXT NOT NULL DEFAULT '[]';
