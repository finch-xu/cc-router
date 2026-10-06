-- url_params: user-supplied provider params (account_id etc), JSON object
ALTER TABLE subscriptions ADD COLUMN url_params TEXT NOT NULL DEFAULT '{}';
-- systemone_wire: System One upstream dialect snapshot (standard or cloudflare_run)
ALTER TABLE subscriptions ADD COLUMN systemone_wire TEXT NOT NULL DEFAULT 'standard';
