//! sweepr: moves old caches and trash into a quarantine you can undo,
//! then deletes them for good after a grace period.

pub mod app;
pub mod commands;
pub mod config;
pub mod disk;
pub mod manifest;
pub mod notify;
pub mod paths;
pub mod progress;
pub mod quarantine;
pub mod report;
pub mod rules;
pub mod safety;
pub mod scan;
