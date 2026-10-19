DROP TABLE vectors;
CREATE VIRTUAL TABLE vectors USING vec0(source_id text partition key,embedding float[384] distance_metric=cosine,chunk_size=8);
DELETE FROM embedding_memberships;
UPDATE documents SET parser_version=0;
PRAGMA user_version=7;
