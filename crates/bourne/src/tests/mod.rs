//! Internal tests — the modules that need crate visibility (the original
//! §6 test split). The former `lib.rs` inline block moved to
//! `tests/api_smoke.rs` (audit 2026-09 F2).

mod escape_decode;
mod float_fast_path;
mod float_uncentred;
mod integer_paths;
mod sink_adapter;
mod sink_direct;
mod stack_frames;
mod unsafe_boundary;
mod vec_fast_path;
