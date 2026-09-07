CREATE TABLE memories (
  id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
INSERT INTO memories VALUES ('memory-1', 'Synthetic migration fixture memory.', 1);
