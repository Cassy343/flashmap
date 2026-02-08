#!/usr/bin/env bash
cargo +stable test
cargo +nightly miri test -- --nocapture
RUST_BACKTRACE=full cargo +stable test --features loom --test loom --profile loomtest -- --nocapture