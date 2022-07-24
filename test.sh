#!/usr/bin/env bash
cargo test
cargo miri test -- --nocapture
RUST_BACKTRACE=full RUSTFLAGS="--cfg loom" cargo test --test loom --profile loomtest -- --nocapture