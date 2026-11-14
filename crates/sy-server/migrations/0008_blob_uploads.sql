-- SPDX-FileCopyrightText: 2026 The Stintory Contributors
-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Which user has proven to have a telemetry file (uploaded it in a sync or
-- an import). A lap may only refer to such a file, so knowing a hash (e.g.
-- from a lap that was shared and is not anymore) gives no access to it.
CREATE TABLE blob_uploads (
    user_id  uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    hash     text NOT NULL REFERENCES blobs (hash) ON DELETE CASCADE,
    PRIMARY KEY (user_id, hash)
);

-- existing laps were uploaded by their owners
INSERT INTO blob_uploads (user_id, hash)
SELECT DISTINCT owner_id, blob_hash FROM laps
ON CONFLICT DO NOTHING;
