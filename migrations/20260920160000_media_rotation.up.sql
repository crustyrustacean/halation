ALTER TABLE media ADD COLUMN rotation INT NOT NULL DEFAULT 0;  -- cumulative manual rotation
ALTER TABLE media ADD COLUMN version  INT NOT NULL DEFAULT 1;  -- cache-buster for derivative URLs
