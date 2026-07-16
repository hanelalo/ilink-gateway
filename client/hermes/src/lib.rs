//! wechat-gateway Hermes client
//!
//! A Rust client that connects to the wechat-gateway and Hermes ACP.
//! Polls the gateway for WeChat messages, forwards them to Hermes via ACP,
//! and sends replies back to the gateway.
//!
//! Architecture:
//! ```text
//! WeChat ←iLink→ wechat-gateway
//!                     │
//!               REST API (register, poll, reply)
//!                     │
//!     ┌───────────────┴───────────────┐
//!     │   Hermes Client (this crate)  │
//!     │                               │
//!     │   ┌───────────────────────┐   │
//!     │   │  ACP session manager   │──│── stdio JSON-RPC → hermes acp
//!     │   └───────────────────────┘   │
//!     │   ┌───────────────────────┐   │
//!     │   │  Gateway API client   │──│── HTTP → wechat-gateway
//!     │   └───────────────────────┘   │
//!     └───────────────────────────────┘
//! ```

pub mod gateway;
pub mod acp;
pub mod client;
pub mod config;
pub mod error;
