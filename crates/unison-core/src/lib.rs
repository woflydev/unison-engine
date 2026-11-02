//! Unison core.
//!
//! Layering, bottom to top:
//! - [`profile`]: the identity profile (which official build we present as), signed and updatable.
//! - [`fidelity`]: the only place request headers, super-properties and IDENTIFY are built.
//! - [`privacy`]: the policy deciding which categories of requests are ever sent.
//! - [`transport`]: HTTP/2 + WebSocket with the Chromium connection profile.
//! - [`rest`] / [`gateway`] / [`remote_auth`]: protocol clients.
//! - [`model`] / [`store`] / [`cache`]: lenient entities, in-memory state with deltas, SQLite.
//! - [`session`]: one signed-in account; [`engine`]: all accounts plus power state.

pub use bytes::Bytes;

pub mod activity;
pub mod cache;
pub mod comfort;
pub mod mic_access;
pub mod dedupe;
pub mod commands;
pub mod emoji;
pub mod emoji_catalog;
pub mod engine;
pub mod error;
pub mod fidelity;
pub mod find;
pub mod forum;
pub mod gateway;
pub mod folders;
pub mod guild_draft;
pub mod guild_admin;
pub mod guild_events;
pub mod intent;
pub mod arrival;
pub mod jump_present;
pub mod jump_scroll;
pub mod interactions;
pub mod localstate;
pub mod msgcodec;
pub mod media_url;
pub mod model;
pub mod moderation;
pub mod navstate;
pub mod open_target;
pub mod poll_create;
pub mod power;
pub mod prefs;
pub mod reaction_info;
pub mod reply;
pub mod privacy;
pub mod profile;
pub mod remote_auth;
pub mod rest;
pub mod saved;
pub mod search_syntax;
pub mod send_rules;
pub mod send_state;
pub mod server_join;
pub mod server_setup;
pub mod session;
pub mod snowflake;
pub mod store;
pub mod subscriptions;
pub mod suggest;
pub mod timefmt;
pub mod upload_policy;
pub mod cosmetics;
pub mod server_card;
pub mod keybinds;
pub mod sticker_motion;
pub mod transport;
pub mod update;
pub mod update_install;
pub mod voice;
pub mod voice_note;
pub mod welcome;
pub mod wsraw;

pub use error::{Error, Result};
