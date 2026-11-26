//! MagicRemoteService: lets an LG webOS TV app drive a Windows PC's mouse and keyboard.
//!
//! The pure core (`protocol`, `ws`, `bindings`, `session`) has no Windows calls; `win` holds them.

pub mod bindings;
pub mod client;
pub mod http;
pub mod ipk;
pub mod log;
pub mod protocol;
pub mod server;
pub mod session;
pub mod settings;
pub mod ssdp;
pub mod state;
pub mod tv;
pub mod win;
pub mod worker;
pub mod ws;
