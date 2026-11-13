-- The org's project standards, enforced here rather than remembered. See `prova-project-standards`.
--
-- Profiles are detected from what is on disk, so this file does not name them. A violation reads
-- like any other test failure, and a deliberate exception is a waiver WITH A REASON.

local standards = require("project-standards")

-- An Anemnez SDK library (User, 2026-11-05: every SDK crate is MIT OR Apache-2.0), so it declares
-- that licence instead of the org's proprietary default.
standards.check { license = "MIT OR Apache-2.0" }
