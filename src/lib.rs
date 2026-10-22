// `#[derive(new)]` (derive-new 0.7, the latest) writes `Self { field: field }` using the field's own span, so clippy 0.1.99 reports
// each field of every struct that derives it as a redundant field name, though nothing was written that way.
#![allow(clippy::redundant_field_names)]

pub mod adapters;
pub mod application;
pub mod bootstrap;
#[cfg(test)]
pub(crate) mod captured_log;
pub mod cli;
pub mod config;
#[cfg(test)]
mod contract_tests;
pub mod domain;
pub mod quiet_times;
pub mod scheduler;
