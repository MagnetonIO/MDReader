CREATE TABLE reader_agent_sessions_meta(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL);
CREATE TABLE reader_agent_sessions(id TEXT PRIMARY KEY,project_id TEXT NOT NULL,connection_id TEXT NOT NULL,client_session_id TEXT NOT NULL,agent TEXT NOT NULL,title TEXT NOT NULL,started_at_ms INTEGER NOT NULL,last_seen_ms INTEGER NOT NULL,ended_at_ms INTEGER,UNIQUE(project_id,connection_id,client_session_id));
CREATE TABLE reader_agent_documents(session_id TEXT NOT NULL,path TEXT NOT NULL,reported_at_ms INTEGER NOT NULL,PRIMARY KEY(session_id,path));
