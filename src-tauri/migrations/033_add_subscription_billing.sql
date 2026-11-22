-- How the subscription is paid for: 'metered' (pay per token, default) or 'flat' (monthly plan such as a
-- Coding Plan). Pricing plugins compute costs for both; flat-rate costs are shown as "equivalent metered cost"
-- and never added to the real spend.
ALTER TABLE subscriptions ADD COLUMN billing TEXT NOT NULL DEFAULT 'metered'
