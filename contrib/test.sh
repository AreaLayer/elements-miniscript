#!/bin/sh

set -ex

FEATURES="compiler serde rand base64"

cargo --version
rustc --version

# Format if told to
if [ "$DO_FMT" = true ]
then
    rustup component add rustfmt
    cargo fmt -- --check
fi

# Pin dependencies required to build with Rust 1.41.1
if cargo --version | grep "1\.41\.0"; then
    cargo update -p once_cell --precise 1.13.1
fi

# Pin dependencies required to build with Rust 1.47.0
if cargo --version | grep "1\.47\.0"; then
    cargo update -p once_cell --precise 1.13.1
fi

# Fuzz if told to
if [ "$DO_FUZZ" = true ]
then
    cd fuzz
    cargo test --verbose
    ./travis-fuzz.sh

    # Exit out of the fuzzer, do not run other tests.
    exit 0
fi

# Test bitcoind integration tests if told to (this only works with the stable toolchain)
if [ "$DO_BITCOIND_TESTS" = true ]; then
    cd bitcoind-tests
    cargo test --verbose

    # Exit integration tests, do not run other tests.
    exit 0
fi

# Defaults / sanity checks
cargo test

if [ "$DO_FEATURE_MATRIX" = true ]
then
    # All features
    cargo test --features="$FEATURES"

    # Single features
    for feature in ${FEATURES}
    do
        cargo test --features="$feature"
    done

    # Run all the examples
    cargo build --examples
    cargo run --example htlc --features=compiler
    cargo run --example parse
    cargo run --example sign_multisig
    cargo run --example verify_tx > /dev/null
    cargo run --example xpub_descriptors
    cargo run --example taproot --features=compiler
    cargo run --example psbt_sign_finalize --features=base64
fi

# Bench if told to (this only works with the nightly toolchain)
if [ "$DO_BENCH" = true ]
then
    cargo bench --features="unstable compiler"
fi

# Build the docs if told to (this only works with the nightly toolchain)
if [ "$DO_DOCS" = true ]; then
    RUSTDOCFLAGS="--cfg docsrs" cargo +nightly rustdoc --features="$FEATURES" -- -D rustdoc::broken-intra-doc-links
fi

exit 0
