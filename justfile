set shell := ["bash", "-euo", "pipefail", "-c"]

# Install the pinned Rust toolchain and fetch locked project dependencies.
setup:
    rustup toolchain install --no-self-update
    cargo fetch --locked

# Run all checks required by CI.
check:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --all-targets --all-features --locked
