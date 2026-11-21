-- Network timing of each attempt (proxy/net_timing.rs). All nullable, old rows stay NULL.
-- dns / connect are only set when the attempt opened a new connection (connect = TCP + TLS).
ALTER TABLE requests ADD COLUMN net_prep_ms INTEGER;
ALTER TABLE requests ADD COLUMN net_dns_ms INTEGER;
ALTER TABLE requests ADD COLUMN net_connect_ms INTEGER;
ALTER TABLE requests ADD COLUMN net_headers_ms INTEGER;
ALTER TABLE requests ADD COLUMN net_conn_reused INTEGER;
ALTER TABLE requests ADD COLUMN net_via_proxy INTEGER;
ALTER TABLE requests ADD COLUMN net_remote_addr TEXT;
ALTER TABLE requests ADD COLUMN net_upstream_stream INTEGER;
