// lib.rs - nyx-app: the Tauri-free app logic. src-tauri (the desktop shell) and later a headless
// daemon both build on AppState and its helpers.

pub mod dto;
pub mod feeds;
pub mod http;
pub mod netlock;
pub mod poller;
pub mod profiles;
pub mod secret;
pub mod state;
pub mod update;
pub mod rotation;
