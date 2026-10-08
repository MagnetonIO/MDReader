CREATE TABLE native_sync_accounts_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version>0));
CREATE TABLE native_sync_account_binding(singleton INTEGER PRIMARY KEY CHECK(singleton=1),remote_library TEXT NOT NULL CHECK(length(remote_library)=36),credential_reference TEXT NOT NULL CHECK(length(credential_reference)=51));
