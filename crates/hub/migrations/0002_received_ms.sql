-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Windows, pruning and "latest" go by the hub's own clock (decision
-- 0071): an agent's clock may be skewed or stepped backward, and the
-- hub must never mix the two. Rows filed before this migration have
-- only the agent's time, which becomes their receive time.
ALTER TABLE samples ADD COLUMN received_ms INTEGER NOT NULL DEFAULT 0;
UPDATE samples SET received_ms = ts_ms;
CREATE INDEX samples_host_received ON samples (host, received_ms);
