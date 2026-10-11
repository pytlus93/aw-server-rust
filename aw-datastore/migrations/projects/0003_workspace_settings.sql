ALTER TABLE pt_workspace ADD COLUMN settings_json TEXT NOT NULL
    DEFAULT '{"name":"Projects","reporting_timezone":"UTC"}';
